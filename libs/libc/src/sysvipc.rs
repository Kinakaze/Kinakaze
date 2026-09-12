//! System V semaphores and shared memory.
//!
//! Windows has real equivalents for both *mechanisms* — a file mapping object
//! is shared memory and a named mutex is a cross-process lock — but none for
//! the System V *namespace*, where a bare integer key names an object that
//! outlives every process that touched it. Rebuilding that namespace is most
//! of the work in this module, and it is where the divergences live, so they
//! are stated up front rather than left to be discovered.
//!
//! # A segment does not outlive its last handle
//!
//! This is the difference that will actually bite a caller. A Windows section
//! object is destroyed when the last handle to it closes. A System V segment
//! marked `IPC_RMID` survives until the last process detaches, which this
//! module reproduces exactly — but a segment *not* marked for removal outlives
//! every process that used it, and that cannot be reproduced at all. A program
//! that writes a segment, exits, and expects the next run to find its data will
//! find a freshly zeroed segment instead. Nothing here can hold the section open
//! once no process does.
//!
//! # The key namespace is `Local\`, and it is per-session
//!
//! Two processes that pass the same key must reach the same object, so the name
//! is derived from the key deterministically: the 32-bit key is rendered as
//! eight lowercase hex digits after a fixed prefix, as in
//! `Local\kinakaze.shm.1.0000002a`. The mapping is injective — every key has
//! exactly one name and no two keys share one — so a collision is impossible by
//! construction rather than merely unlikely, which a hash could not promise.
//!
//! `Local\` places the objects in the caller's Terminal Services session. The
//! cost is that two kinakaze processes in *different* sessions (an interactive
//! login and a service, say) will not see each other's segments even with the
//! same key. `Global\` would fix that, but creating a global object requires
//! `SeCreateGlobalPrivilege`, which an ordinary user does not hold, so choosing
//! it would trade a rare limitation for a common failure.
//!
//! # Semaphores are real, with two named exceptions
//!
//! A System V semaphore set is not a Windows semaphore and cannot be built from
//! any number of them: `semop` takes an *array* precisely because the whole
//! array must apply atomically or not at all, and independent Windows semaphores
//! cannot promise that. The set is instead a shared header guarded by one named
//! mutex, and `semop` decides feasibility for every operation before applying
//! any of them. Blocking on zero, which has no Windows primitive at all, falls
//! out of the same design. `SEM_UNDO` is implemented and is crash-safe; see
//! [`reclaim_dead_undo`].
//!
//! Refused with `ENOSYS`: `GETNCNT` and `GETZCNT`. A blocking `semop` is also
//! not interruptible by a signal, so it never reports `EINTR`.
use core::alloc::{GlobalAlloc, Layout};
use core::ffi::{CStr, c_char, c_int, c_void};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
mod lifecycle;

use kinakaze_vfs::{
    EAGAIN, EEXIST, EFAULT, EINVAL, EIO, ENOENT, ENOMEM, ENOSPC, ENOSYS, ERANGE, errno_from_win32,
};

/// `EIDRM`, which the VFS error list does not carry.
///
/// Reported when an identifier names an object that has been removed, which is
/// the one error every System V caller distinguishes from `EINVAL`: `EIDRM`
/// means "it was there and is gone", and a caller that polls a set treats it as
/// a clean shutdown rather than as a bug in its own bookkeeping.
pub const EIDRM: i32 = 43;

use crate::set_errno;

/// Linux `key_t`: a signed 32-bit integer on every Linux ABI.
pub type Key = i32;

/// `IPC_PRIVATE`, the key that means "a new object nobody can name".
pub const IPC_PRIVATE: Key = 0;

// `shmget`/`semget` flag bits, which live in the low nine bits' neighbours.
pub const IPC_CREAT: c_int = 0o1000;
pub const IPC_EXCL: c_int = 0o2000;
pub const IPC_NOWAIT: c_int = 0o4000;

// `shmctl`/`semctl` commands.
pub const IPC_RMID: c_int = 0;
pub const IPC_SET: c_int = 1;
pub const IPC_STAT: c_int = 2;

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_msgctl(
    _id: c_int,
    _command: c_int,
    _buffer: *mut c_void,
) -> c_int {
    // The hosted IPC provider implements semaphores and shared memory, but no
    // System V message queues. Do not claim empty host statistics as real data.
    set_errno(ENOSYS);
    -1
}

// `semctl` commands beyond the three shared ones.
pub const GETPID: c_int = 11;
pub const GETVAL: c_int = 12;
pub const GETALL: c_int = 13;
pub const GETNCNT: c_int = 14;
pub const GETZCNT: c_int = 15;
pub const SETVAL: c_int = 16;
pub const SETALL: c_int = 17;

/// `SEM_UNDO`, a `sembuf.sem_flg` bit requesting reversal on process exit.
pub const SEM_UNDO: c_int = 0x1000;

// `shmat` flags.
pub const SHM_RDONLY: c_int = 0o10000;
/// `SHM_RND`: round a requested attach address down instead of failing.
pub const SHM_RND: c_int = 0o20000;

/// `SHMLBA`, the address multiple `SHM_RND` rounds to.
///
/// Linux uses the page size. Here it is the Windows allocation granularity,
/// because that is the only boundary `MapViewOfFileEx` accepts a base address
/// on, and reporting a finer one would let `SHM_RND` produce an address the host
/// then refuses. The value is 64 KiB on every Windows x86_64 system; it is a
/// constant because `SHMLBA` is one on Linux too, and [`control_bytes`] asks the
/// host for the real granularity rather than trusting this number.
pub const SHMLBA: usize = 65536;

/// `SEMVMX`, the largest value a semaphore may hold. Linux's own ceiling.
pub const SEMVMX: i32 = 32767;

/// `SEMMSL`, the most semaphores one set may contain. Linux's default.
pub const SEMMSL: usize = 250;

/// `SEMOPM`, the most operations one `semop` call may carry. Linux's default.
pub const SEMOPM: usize = 500;

/// Undo slots per set, bounding the shared header.
///
/// Linux sizes its undo lists dynamically; a fixed shared header cannot, so the
/// table holds this many `(process, semaphore)` pairs and `semop` reports
/// `ENOSPC` when it fills. That is the error Linux itself returns when its undo
/// structure cannot be allocated, so callers already handle it.
const UNDO_SLOTS: usize = 128;
/// Linux x86_64 `struct ipc_perm`, as glibc declares it.
///
/// Layout is ABI: guest code reads these offsets out of a `shmid_ds` it passed
/// to `shmctl`. The kernel's `ipc64_perm` spells `mode` as a 32-bit word where
/// glibc spells it as `unsigned short` plus `__pad1`; the two agree on a
/// little-endian machine for every mode a caller can express, and glibc's
/// spelling is the one used here because the guest was compiled against glibc.
///
/// ```text
///   0  key_t          __key
///   4  uid_t          uid
///   8  gid_t          gid
///  12  uid_t          cuid
///  16  gid_t          cgid
///  20  unsigned short mode
///  22  unsigned short __pad1
///  24  unsigned short __seq
///  26  unsigned short __pad2
///  32  unsigned long  __unused1
///  40  unsigned long  __unused2
///  48  (end)
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IpcPerm {
    pub key: Key,
    pub uid: u32,
    pub gid: u32,
    pub cuid: u32,
    pub cgid: u32,
    pub mode: u16,
    pub pad1: u16,
    pub seq: u16,
    pub pad2: u16,
    pub unused1: u64,
    pub unused2: u64,
}

/// Linux x86_64 `struct shmid_ds`.
///
/// ```text
///   0  struct ipc_perm shm_perm      (48 bytes)
///  48  size_t          shm_segsz
///  56  time_t          shm_atime
///  64  time_t          shm_dtime
///  72  time_t          shm_ctime
///  80  pid_t           shm_cpid
///  84  pid_t           shm_lpid
///  88  unsigned long   shm_nattch
///  96  unsigned long   __unused4
/// 104  unsigned long   __unused5
/// 112  (end)
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ShmidDs {
    pub shm_perm: IpcPerm,
    pub shm_segsz: usize,
    pub shm_atime: i64,
    pub shm_dtime: i64,
    pub shm_ctime: i64,
    pub shm_cpid: c_int,
    pub shm_lpid: c_int,
    pub shm_nattch: u64,
    pub unused4: u64,
    pub unused5: u64,
}

/// Linux x86_64 `struct semid_ds`.
///
/// ```text
///   0  struct ipc_perm sem_perm      (48 bytes)
///  48  time_t          sem_otime
///  56  time_t          sem_ctime
///  64  unsigned long   sem_nsems
///  72  unsigned long   __unused3
///  80  unsigned long   __unused4
///  88  (end)
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SemidDs {
    pub sem_perm: IpcPerm,
    pub sem_otime: i64,
    pub sem_ctime: i64,
    pub sem_nsems: u64,
    pub unused3: u64,
    pub unused4: u64,
}

/// Linux `struct sembuf`, the element type of the `semop` array.
///
/// Six bytes with two-byte alignment, so an array of them is densely packed and
/// a guest's `sops[2]` lands at offset 12. Getting this wrong would silently
/// shift every operation past the first.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Sembuf {
    pub sem_num: u16,
    pub sem_op: i16,
    pub sem_flg: i16,
}
// Win32 entry points this module needs.
//
// Declared inline rather than imported from `windows-sys` because the crate's
// enabled feature set covers `Win32_System_Threading` only, and the section and
// synchronization calls below live in `Win32_System_Memory` and
// `Win32_System_Threading`'s wider surface. Every signature matches the Windows
// headers: `BOOL` is `i32`, a `HANDLE` is a pointer, and `LPCWSTR` is a
// null-terminated UTF-16 string.
#[link(name = "kernel32")]
unsafe extern "system" {
    /// `CreateFileMappingW`: creates or opens a named section object.
    fn CreateFileMappingW(
        file: *mut c_void,
        attributes: *const SecurityAttributes,
        protect: u32,
        maximum_size_high: u32,
        maximum_size_low: u32,
        name: *const u16,
    ) -> *mut c_void;
    /// `OpenFileMappingW`: opens an existing named section, without creating.
    fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
    /// `MapViewOfFile`: maps a section into this process at an address of the
    /// kernel's choosing.
    fn MapViewOfFile(
        mapping: *mut c_void,
        access: u32,
        offset_high: u32,
        offset_low: u32,
        bytes: usize,
    ) -> *mut c_void;
    /// `MapViewOfFileEx`: the same, at a caller-chosen base address.
    fn MapViewOfFileEx(
        mapping: *mut c_void,
        access: u32,
        offset_high: u32,
        offset_low: u32,
        bytes: usize,
        base: *mut c_void,
    ) -> *mut c_void;
    /// `UnmapViewOfFile`: releases one mapped view.
    fn UnmapViewOfFile(base: *const c_void) -> i32;
    /// `CreateMutexW`: creates or opens a named mutex.
    fn CreateMutexW(attributes: *const c_void, owned: i32, name: *const u16) -> *mut c_void;
    /// `ReleaseMutex`: drops ownership acquired by a successful wait.
    fn ReleaseMutex(mutex: *mut c_void) -> i32;
    /// `CreateEventW`: creates or opens a named event.
    fn CreateEventW(
        attributes: *const c_void,
        manual_reset: i32,
        initial: i32,
        name: *const u16,
    ) -> *mut c_void;
    /// `SetEvent`: signals an event.
    fn SetEvent(event: *mut c_void) -> i32;
    /// `WaitForSingleObject`: waits with a millisecond timeout.
    fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
    /// `CloseHandle`: releases a kernel handle.
    fn CloseHandle(handle: *mut c_void) -> i32;
    /// `GetLastError`: the failure code of the last failing call.
    fn GetLastError() -> u32;
    /// `GetSystemInfo`: queried for the real allocation granularity.
    fn GetSystemInfo(info: *mut SystemInfo);
    /// `GetCurrentProcess`: the pseudo-handle for this process.
    fn GetCurrentProcess() -> *mut c_void;
    /// `OpenProcess`: a handle to another process, for the liveness test.
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    /// `GetProcessTimes`: read for its creation time, which distinguishes a
    /// live process from a recycled process id.
    fn GetProcessTimes(
        process: *mut c_void,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
}

/// `SECURITY_ATTRIBUTES`. Never populated: every object here takes the default
/// descriptor, so only a null pointer is ever passed.
///
/// Declared rather than using `*const c_void` so the signature matches the other
/// `CreateFileMappingW` declaration in this crate; two declarations of one import
/// that disagree are a warning, and rightly so.
#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: i32,
}

/// `FILETIME`, a count of 100-nanosecond ticks split across two words.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FileTime {
    low: u32,
    high: u32,
}

impl FileTime {
    /// Reassembles the split halves, giving a value that can be compared for
    /// equality against another process's creation time.
    fn ticks(self) -> u64 {
        (u64::from(self.high) << 32) | u64::from(self.low)
    }
}

/// `SYSTEM_INFO`. Only `allocation_granularity` is read.
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

const PAGE_READWRITE: u32 = 0x0000_0004;
const FILE_MAP_READ: u32 = 0x0000_0004;
const FILE_MAP_WRITE: u32 = 0x0000_0002;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_ABANDONED: u32 = 0x0000_0080;
const WAIT_TIMEOUT: u32 = 0x0000_0102;
const INFINITE: u32 = 0xFFFF_FFFF;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_FILE_NOT_FOUND: u32 = 2;
fn last_errno() -> i32 {
    // SAFETY: GetLastError has no preconditions.
    errno_from_win32(unsafe { GetLastError() })
}

/// Unix seconds, for the `*_time` fields of the two `*id_ds` structures.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// This process's id, as exposed through Linux ABI structures and commands.
fn own_pid() -> c_int {
    crate::process::kinakaze_abi_getpid()
}

/// The real Windows id used only for host process liveness and start tokens.
fn own_host_pid() -> u32 {
    kinakaze_runtime::job::current_host_pid()
}

/// The host's allocation granularity, which bounds where a view may start.
fn allocation_granularity() -> usize {
    static GRANULARITY: OnceLock<usize> = OnceLock::new();
    *GRANULARITY.get_or_init(|| {
        let mut info = SystemInfo::default();
        // SAFETY: `info` is a writable local of the right type; the call cannot
        // fail.
        unsafe { GetSystemInfo(&raw mut info) };
        match info.allocation_granularity as usize {
            // A zero would mean the query failed, in which case the documented
            // x86_64 value is the only number available.
            0 => SHMLBA,
            real => real,
        }
    })
}

/// Bytes reserved at the front of every section for its cross-process header.
///
/// A view may only begin at a multiple of the allocation granularity, so the
/// header occupies a whole granule and the guest's data begins at the next one.
/// That is what lets the header stay mapped read-write for bookkeeping while the
/// data view is mapped read-only for an `SHM_RDONLY` attach: they are two
/// separate views of one section rather than one view the guest could write
/// through.
///
/// The cost is one granule — 64 KiB of address space and commit charge — per
/// segment. Paying it buys a truthful `shmctl(IPC_STAT)`: without a shared
/// header, `shm_segsz` for a segment this process did not create could only be
/// recovered from `VirtualQuery`, which rounds up to a page, and `shm_nattch`
/// could only ever count this process's own attachments.
fn control_bytes() -> usize {
    let granularity = allocation_granularity();
    // Both headers are far smaller than one granule, but the arithmetic is
    // written out so an added field cannot silently overlap the data.
    let needed = size_of::<ShmHeader>().max(size_of::<SemHeader>());
    needed.div_ceil(granularity) * granularity
}

/// Renders a key as the Windows object name that must be identical in every
/// process.
///
/// Split out from the wide-string conversion so the derivation itself is
/// testable: it is the one piece of this module that two processes have to agree
/// on exactly, and a change to it silently stops them sharing.
fn ipc_namespace() -> u64 {
    kinakaze_vfs::namespaces::current_id(kinakaze_vfs::namespaces::IPC).unwrap_or(u64::MAX)
}

fn object_name(kind: &str, key: Key) -> String {
    object_name_in(kind, key, ipc_namespace())
}
fn object_name_in(kind: &str, key: Key, namespace: u64) -> String {
    // The key is formatted from its unsigned bit pattern so a negative key —
    // which `ftok` readily produces — yields a name with no sign character, and
    // so the mapping stays injective across the whole 32-bit range.
    format!(
        "Local\\kinakaze.{kind}.ns{}.{}.{:08x}",
        kinakaze_runtime::authority::domain_id(),
        namespace,
        key as u32
    )
}

/// A null-terminated UTF-16 copy of `text`, for the `*W` entry points.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Holds a named mutex for as long as the guard lives.
///
/// Every mutation of a shared header happens under one of these, which is what
/// makes `semop` atomic and what keeps a segment's creation and initialization
/// from being observed half-done by a second process.
struct CrossProcessLock {
    mutex: *mut c_void,
    owned: bool,
}

impl CrossProcessLock {
    /// Acquires the named mutex `name`, creating it if it does not exist.
    ///
    /// A mutex abandoned by a process that died while holding it is *acquired*,
    /// not refused: `WAIT_ABANDONED` means this thread now owns it and the
    /// protected state may be torn. The headers here are only ever mutated
    /// through short critical sections that leave them consistent at every
    /// instruction boundary a crash could fall on, so continuing is safe and
    /// refusing would wedge the key permanently.
    fn acquire(name: Option<&str>) -> Result<Self, i32> {
        let wide_name = name.map(wide);
        let name_ptr = wide_name
            .as_ref()
            .map_or(std::ptr::null(), |units| units.as_ptr());
        // SAFETY: a null attribute pointer requests the default descriptor, and
        // `name_ptr` is either null or a null-terminated wide string.
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name_ptr) };
        if mutex.is_null() {
            return Err(last_errno());
        }
        // SAFETY: the handle was just created.
        match unsafe { WaitForSingleObject(mutex, INFINITE) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Self { mutex, owned: true }),
            _ => {
                // SAFETY: owned here and not stored.
                unsafe { CloseHandle(mutex) };
                Err(EIO)
            }
        }
    }

    /// Acquires an already-open mutex handle, which the guard does not own.
    fn acquire_handle(mutex: *mut c_void) -> Result<Self, i32> {
        // SAFETY: the caller passes a live mutex handle.
        match unsafe { WaitForSingleObject(mutex, INFINITE) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Self {
                mutex,
                owned: false,
            }),
            _ => Err(EIO),
        }
    }
}

impl Drop for CrossProcessLock {
    fn drop(&mut self) {
        // SAFETY: this guard exists only after a successful wait, so this thread
        // holds the mutex.
        unsafe { ReleaseMutex(self.mutex) };
        if self.owned {
            // SAFETY: the handle was created by `acquire` and is not shared.
            unsafe { CloseHandle(self.mutex) };
        }
    }
}
/// `ShmHeader.magic`, distinguishing an initialized header from fresh zeroes.
///
/// A section is zero-filled on creation, so a zero magic is exactly the state a
/// creator that died between `CreateFileMappingW` and initialization leaves
/// behind, and the next process to hold the key's mutex re-initializes it.
const SHM_MAGIC: u64 = 0x4352_5953_5348_4d01;

/// `SemHeader.magic`, with the same role.
const SEM_MAGIC: u64 = 0x4352_5953_5345_4d01;

/// The cross-process state of one shared-memory segment.
///
/// Lives in the first granule of the segment's own section, so every process
/// that can reach the segment reads the same attachment count, size and
/// timestamps. Field order is this module's own business — no guest ever sees
/// this layout — but every process must agree on it, which is why the magic
/// carries a version digit.
#[repr(C)]
struct ShmHeader {
    magic: u64,
    /// The size the creator asked for, not the rounded section size. This is the
    /// number `shm_segsz` must report.
    segsz: u64,
    nattch: u64,
    atime: i64,
    dtime: i64,
    ctime: i64,
    cpid: i32,
    lpid: i32,
    key: i32,
    mode: u32,
    /// Set by `IPC_RMID`. A removed segment stays usable for processes already
    /// attached and is invisible to `shmget`, exactly as on Linux.
    removed: u32,
    pad: u32,
}

/// One undo record: the adjustment one process owes one semaphore.
///
/// `pid` and `started` together identify the process. The creation time is part
/// of the identity because Windows recycles process ids, and reversing a live
/// process's adjustments because it happens to hold a dead one's id would be
/// worse than not reversing them at all.
#[repr(C)]
#[derive(Clone, Copy)]
struct UndoEntry {
    /// Zero when the slot is free.
    pid: u32,
    sem_num: u32,
    started: u64,
    adjust: i32,
    pad: u32,
}

/// The cross-process state of one semaphore set.
#[repr(C)]
struct SemHeader {
    magic: u64,
    otime: i64,
    ctime: i64,
    nsems: u32,
    key: i32,
    mode: u32,
    removed: u32,
    /// Current values. Held as `i32` so an operation can be range-checked
    /// against `SEMVMX` before it is applied; the guest only ever sees these
    /// through `GETVAL`/`GETALL`, which narrow to `unsigned short`.
    values: [i32; SEMMSL],
    /// `sempid`: the last process to operate on each semaphore.
    last_pid: [i32; SEMMSL],
    undo: [UndoEntry; UNDO_SLOTS],
}

/// One segment this process knows about, keyed by the id `shmget` returned.
struct Segment {
    namespace: u64,
    key: Key,
    /// The section handle. Closing it is what can destroy the section.
    mapping: usize,
    fork_slot: usize,
    private_lock: u64,
    /// A read-write view of the header granule, held for the segment's lifetime.
    control: usize,
    segsz: usize,
}

impl Drop for Segment {
    fn drop(&mut self) {
        let _mapping = kinakaze_runtime::begin_fork_mapping_transaction();
        kinakaze_runtime::unregister_fork_handle_slot(self.fork_slot as _);
        // SAFETY: both were produced by this module's own successful calls and
        // are released exactly once, when the last `Arc` to this segment goes.
        unsafe {
            UnmapViewOfFile(self.control as *const c_void);
            CloseHandle(self.mapping as *mut c_void);
            kinakaze_alloc::ManagedAllocator.dealloc(
                self.fork_slot as *mut u8,
                Layout::new::<std::sync::atomic::AtomicUsize>(),
            );
        }
    }
}

/// One semaphore set this process knows about.
///
/// Unlike [`Segment`], this holds its mutex open rather than reopening it by
/// name for each operation: `semop` may block for a long time under it, so the
/// handle is worth keeping, and an unnamed `IPC_PRIVATE` set has no name to
/// reopen from in the first place.
struct SemSet {
    namespace: u64,
    mapping: usize,
    control: usize,
    /// The named mutex serializing every operation on this set.
    lock: usize,
    /// The auto-reset event a completed operation signals so one blocked waiter
    /// re-tests its condition.
    wake: usize,
    nsems: usize,
}

impl Drop for SemSet {
    fn drop(&mut self) {
        // SAFETY: all four were produced by this module and are released once.
        unsafe {
            UnmapViewOfFile(self.control as *const c_void);
            CloseHandle(self.mapping as *mut c_void);
            CloseHandle(self.lock as *mut c_void);
            CloseHandle(self.wake as *mut c_void);
        }
    }
}

/// Everything this process tracks for both mechanisms.
///
/// The ids handed to the guest are indices into these maps, which is a real
/// divergence: a System V identifier is system-wide and can be passed to another
/// process, whereas one of these is meaningless outside the process that
/// received it. What *is* preserved is the property callers actually use — two
/// processes calling `shmget` with the same key reach the same segment, each
/// under its own id.
struct IpcState {
    segments: HashMap<c_int, Arc<Segment>>,
    /// Live attachments, mapping the address the guest holds to its segment id.
    /// Kept separately from `segments` so `shmdt` still works on a segment that
    /// `IPC_RMID` has already removed.
    attachments: HashMap<usize, ShmAttachment>,
    sets: HashMap<c_int, Arc<SemSet>>,
    /// Ids withdrawn by `IPC_RMID`, so a later call reports `EIDRM` rather than
    /// `EINVAL`. This is what tells a caller its object was removed by someone
    /// else instead of never having existed.
    removed: HashSet<c_int>,
    next_id: c_int,
}
struct ShmAttachment {
    id: c_int,
    segment: Arc<Segment>,
    read_only: bool,
}

pub fn shm_attachment(address: usize) -> Option<(usize, bool)> {
    state()
        .lock()
        .ok()?
        .attachments
        .get(&address)
        .map(|view| (view.segment.segsz, view.read_only))
}

static STATE: OnceLock<Mutex<IpcState>> = OnceLock::new();

fn state() -> &'static Mutex<IpcState> {
    STATE.get_or_init(|| {
        Mutex::new(IpcState {
            segments: HashMap::new(),
            attachments: HashMap::new(),
            sets: HashMap::new(),
            removed: HashSet::new(),
            // Ids start at 1 and are never reused, so a stale id can never name
            // a different object than the one its holder means.
            next_id: 1,
        })
    })
}

/// Reports an error through errno and returns the -1 every entry point here
/// uses for failure.
fn fail(error: i32) -> c_int {
    set_errno(error);
    -1
}
// ---------------------------------------------------------------------------
// Shared memory.
// ---------------------------------------------------------------------------

/// Creates or opens a section, reporting whether it already existed.
///
/// `name` is `None` for `IPC_PRIVATE`, which wants an unnamed section: nobody
/// can name it, which is exactly the guarantee `IPC_PRIVATE` makes, so the
/// Windows and Linux behaviours coincide with nothing to reconcile.
fn create_section(name: Option<&str>, bytes: u64) -> Result<(*mut c_void, bool), i32> {
    let wide_name = name.map(wide);
    let name_ptr = wide_name
        .as_ref()
        .map_or(std::ptr::null(), |units| units.as_ptr());
    // SAFETY: an INVALID_HANDLE_VALUE file requests a pagefile-backed section, a
    // null attribute pointer the default descriptor, and `name_ptr` is null or a
    // null-terminated wide string.
    let mapping = unsafe {
        CreateFileMappingW(
            usize::MAX as *mut c_void,
            std::ptr::null(),
            PAGE_READWRITE,
            (bytes >> 32) as u32,
            (bytes & 0xffff_ffff) as u32,
            name_ptr,
        )
    };
    if mapping.is_null() {
        return Err(last_errno());
    }
    // Read before any other call can overwrite it. `CreateFileMappingW` succeeds
    // for an existing name and reports the fact only here.
    // SAFETY: GetLastError has no preconditions.
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    Ok((mapping, existed))
}

/// Maps the header granule of a section read-write.
fn map_control(mapping: *mut c_void) -> Result<*mut c_void, i32> {
    // SAFETY: `mapping` is a live section handle and the view is bounded by the
    // header granule, which every section this module creates contains.
    let view = unsafe {
        MapViewOfFile(
            mapping,
            FILE_MAP_READ | FILE_MAP_WRITE,
            0,
            0,
            control_bytes(),
        )
    };
    if view.is_null() {
        return Err(last_errno());
    }
    Ok(view)
}

/// `shmget`: creates or opens a shared-memory segment.
///
/// `IPC_PRIVATE` produces an unnamed section, which is the clean correspondence
/// noted in the module header. For every other key the section is named from the
/// key, and creation is serialized on a per-key mutex so a second process cannot
/// observe a header between `CreateFileMappingW` and its initialization.
///
/// `IPC_CREAT | IPC_EXCL` reports `EEXIST` when the section already exists, which
/// is how callers arbitrate ownership, so the check is made against the host's
/// own "already existed" answer rather than against this module's bookkeeping —
/// the creator may be another process entirely.
///
/// Two divergences are worth knowing. Opening an existing segment whose size is
/// smaller than `size` reports `EINVAL`, as on Linux. And a segment that
/// `IPC_RMID` has removed but that another process is still attached to keeps its
/// Windows name until that process detaches, so `shmget` on that key reports
/// `EIDRM` during the window instead of creating a fresh segment; on Linux the
/// name is free immediately. The window is bounded by the other process's
/// `shmdt`, and reporting the condition beats handing back a stale segment.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_shmget(key: Key, size: usize, flags: c_int) -> c_int {
    let creating = flags & IPC_CREAT != 0;
    let exclusive = flags & IPC_EXCL != 0;

    // A zero size is only meaningful when opening an existing segment; Linux
    // rejects a create of nothing.
    if size == 0 && creating {
        return fail(EINVAL);
    }
    // The section size must fit the header granule as well as the data. A
    // request that overflows cannot be served by any amount of memory.
    let Some(total) = size
        .checked_add(control_bytes())
        .and_then(|total| u64::try_from(total).ok())
    else {
        return fail(ENOMEM);
    };

    let name = (key != IPC_PRIVATE).then(|| object_name("shm", key));
    // Held across create-and-initialize so the header is never observed
    // half-written. Unnamed sections need no lock: no other process can find
    // them, so there is no second observer to serialize against.
    let _lock = match name.as_deref() {
        Some(_) => match CrossProcessLock::acquire(Some(&object_name("shmlock", key))) {
            Ok(lock) => Some(lock),
            Err(error) => return fail(error),
        },
        None => None,
    };

    let (mapping, existed) = if creating || key == IPC_PRIVATE {
        match create_section(name.as_deref(), total) {
            Ok(pair) => pair,
            Err(error) => return fail(error),
        }
    } else {
        // Without IPC_CREAT the segment must already exist, and `OpenFileMappingW`
        // is the call that says so rather than creating one behind the caller's
        // back.
        let wide_name = wide(name.as_deref().unwrap_or_default());
        // SAFETY: `wide_name` is a null-terminated wide string.
        let mapping =
            unsafe { OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, 0, wide_name.as_ptr()) };
        if mapping.is_null() {
            // SAFETY: GetLastError has no preconditions.
            let error = unsafe { GetLastError() };
            return fail(if error == ERROR_FILE_NOT_FOUND {
                ENOENT
            } else {
                errno_from_win32(error)
            });
        }
        (mapping, true)
    };

    if existed && exclusive && creating {
        // SAFETY: the handle is owned here and not stored.
        unsafe { CloseHandle(mapping) };
        return fail(EEXIST);
    }

    let control = match map_control(mapping) {
        Ok(view) => view,
        Err(error) => {
            // SAFETY: owned here and not stored.
            unsafe { CloseHandle(mapping) };
            return fail(error);
        }
    };

    match initialize_shm_header(control, existed, key, size, flags) {
        Ok(segsz) => match publish_segment(key, mapping, control, segsz) {
            Ok(id) => id,
            Err(error) => {
                release_section(mapping, control);
                fail(error)
            }
        },
        Err(error) => {
            release_section(mapping, control);
            fail(error)
        }
    }
}

/// Unmaps a control view and closes its section on a failure path.
fn release_section(mapping: *mut c_void, control: *mut c_void) {
    // SAFETY: both were produced by this module's successful calls and have not
    // been published to the state table.
    unsafe {
        UnmapViewOfFile(control);
        CloseHandle(mapping);
    }
}

/// Initializes a fresh header, or validates an existing one, returning the size.
///
/// # Safety
///
/// `control` must be a writable mapping of at least one [`ShmHeader`], which
/// [`map_control`] guarantees. The caller must hold the key's mutex.
fn initialize_shm_header(
    control: *mut c_void,
    existed: bool,
    key: Key,
    size: usize,
    flags: c_int,
) -> Result<usize, i32> {
    let header = control.cast::<ShmHeader>();
    // SAFETY: `control` maps a whole granule, which is larger than a ShmHeader,
    // and the key's mutex excludes every other writer.
    let header = unsafe { &mut *header };

    // A zero magic means nobody has initialized this section — either it is new,
    // or its creator died before finishing. Both cases want initialization.
    if !existed || header.magic != SHM_MAGIC {
        let now = now_seconds();
        *header = ShmHeader {
            magic: SHM_MAGIC,
            segsz: size as u64,
            nattch: 0,
            atime: 0,
            dtime: 0,
            ctime: now,
            cpid: own_pid(),
            lpid: 0,
            key,
            // Only the low nine bits are permission bits; the rest are the
            // IPC_CREAT family and are not part of the mode.
            mode: (flags & 0o777) as u32,
            removed: 0,
            pad: 0,
        };
        return Ok(size);
    }

    if header.removed != 0 {
        return Err(EIDRM);
    }
    let existing = header.segsz as usize;
    // Linux reports EINVAL when the caller asks for more than the segment holds,
    // and a silent shrink here would let the caller write past the section.
    if size > existing {
        return Err(EINVAL);
    }
    Ok(existing)
}

/// Records a segment in the state table and returns its id.
fn publish_segment(
    key: Key,
    mapping: *mut c_void,
    control: *mut c_void,
    segsz: usize,
) -> Result<c_int, i32> {
    let mut state = state().lock().map_err(|_| EIO)?;
    let _mapping = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(EIO)?;
    let slot = unsafe {
        kinakaze_alloc::ManagedAllocator.alloc(Layout::new::<std::sync::atomic::AtomicUsize>())
    }
    .cast::<std::sync::atomic::AtomicUsize>();
    if slot.is_null() {
        return Err(ENOMEM);
    }
    unsafe {
        slot.write(std::sync::atomic::AtomicUsize::new(mapping as usize));
    }
    if !unsafe { kinakaze_runtime::register_fork_handle_slot(slot) } {
        unsafe {
            kinakaze_alloc::ManagedAllocator
                .dealloc(slot.cast(), Layout::new::<std::sync::atomic::AtomicUsize>());
        }
        return Err(EIO);
    }
    let id = state.next_id;
    state.next_id += 1;
    state.segments.insert(
        id,
        Arc::new(Segment {
            namespace: ipc_namespace(),
            key,
            mapping: mapping as usize,
            fork_slot: slot as usize,
            private_lock: (u64::from(own_host_pid()) << 32) | id as u32 as u64,
            control: control as usize,
            segsz,
        }),
    );
    Ok(id)
}
/// `(void *)-1`, which `shmat` returns on failure rather than null.
///
/// Null is a legitimate result for `mmap`-family calls on some systems, so
/// System V chose a value that cannot be a valid mapping. A caller comparing
/// against null instead would treat every failure as success.
const SHM_FAILED: *mut c_void = usize::MAX as *mut c_void;

/// Looks up a live segment, distinguishing "removed" from "never existed".
fn find_segment(id: c_int) -> Result<Arc<Segment>, i32> {
    let state = state().lock().map_err(|_| EIO)?;
    if let Some(segment) = state
        .segments
        .get(&id)
        .filter(|s| s.namespace == ipc_namespace())
    {
        return Ok(Arc::clone(segment));
    }
    Err(if state.removed.contains(&id) {
        EIDRM
    } else {
        EINVAL
    })
}

/// `shmat`: maps a segment into this process's address space.
///
/// The returned address is the base of a view of the segment's *data*, which
/// begins one granule into the section; the header granule is never visible to
/// the guest, so a segment of `n` bytes offers exactly `n` writable bytes as it
/// would on Linux.
///
/// `SHM_RDONLY` is genuinely enforced by mapping the data view `FILE_MAP_READ`,
/// so a write through the returned pointer faults as it would on Linux. The
/// bookkeeping the header needs still happens, because the header is a separate
/// read-write view the guest cannot reach.
///
/// A non-null `shmaddr` is honoured through `MapViewOfFileEx`. Windows requires a
/// requested base to be granularity-aligned, so an unaligned address reports
/// `EINVAL` unless `SHM_RND` asks for it to be rounded down — the same contract
/// Linux states, with `SHMLBA` set to the granularity the host can actually
/// satisfy.
///
/// # Safety
///
/// `shmaddr` must be null or an address this process may have mapped.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_shmat(
    id: c_int,
    shmaddr: *const c_void,
    flags: c_int,
) -> *mut c_void {
    let Ok(mut state) = state().lock() else {
        set_errno(EIO);
        return SHM_FAILED;
    };
    let Some(segment) = state
        .segments
        .get(&id)
        .filter(|s| s.namespace == ipc_namespace())
        .cloned()
    else {
        set_errno(if state.removed.contains(&id) {
            EIDRM
        } else {
            EINVAL
        });
        return SHM_FAILED;
    };
    let Some(_mapping) = kinakaze_runtime::begin_fork_mapping_transaction() else {
        set_errno(EIO);
        return SHM_FAILED;
    };

    let read_only = flags & SHM_RDONLY != 0;
    let access = if read_only {
        FILE_MAP_READ
    } else {
        FILE_MAP_READ | FILE_MAP_WRITE
    };

    let mut requested = shmaddr as usize;
    if requested != 0 {
        let granularity = allocation_granularity();
        if !requested.is_multiple_of(granularity) {
            if flags & SHM_RND == 0 {
                set_errno(EINVAL);
                return SHM_FAILED;
            }
            requested -= requested % granularity;
        }
        // The caller names where the *data* should land, and the view it belongs
        // to starts one granule earlier, so an address in the first granule of
        // the address space cannot be served.
        if requested < control_bytes() {
            set_errno(EINVAL);
            return SHM_FAILED;
        }
    }

    let offset = control_bytes() as u64;
    let view = if requested == 0 {
        // SAFETY: the handle is live and the view is bounded by the segment size
        // the header recorded.
        unsafe {
            MapViewOfFile(
                segment.mapping as *mut c_void,
                access,
                (offset >> 32) as u32,
                (offset & 0xffff_ffff) as u32,
                segment.segsz,
            )
        }
    } else {
        // SAFETY: as above; the base is granularity-aligned, which is what
        // MapViewOfFileEx requires.
        unsafe {
            MapViewOfFileEx(
                segment.mapping as *mut c_void,
                access,
                (offset >> 32) as u32,
                (offset & 0xffff_ffff) as u32,
                segment.segsz,
                requested as *mut c_void,
            )
        }
    };
    if view.is_null() {
        set_errno(last_errno());
        return SHM_FAILED;
    }

    if !kinakaze_runtime::register_fork_mapping(kinakaze_runtime::ForkMapping {
        base: view as usize,
        len: segment.segsz.next_multiple_of(4096),
        behavior: kinakaze_runtime::ForkMappingBehavior::Copy,
        storage: kinakaze_runtime::ForkMappingStorage::RetainedSection,
        backing_slot: segment.fork_slot,
        backing_offset: offset,
        view_protection: if read_only { 2 } else { 4 },
        domain: kinakaze_runtime::ForkMappingDomain::GuestMm,
    }) {
        unsafe {
            UnmapViewOfFile(view);
        }
        set_errno(EIO);
        return SHM_FAILED;
    }
    state.attachments.insert(
        view as usize,
        ShmAttachment {
            id,
            segment: segment.clone(),
            read_only,
        },
    );
    drop(state);

    // The attach count and time live in the shared header, so a second process's
    // IPC_STAT sees this attachment too.
    if let Ok(_lock) = shm_lock(&segment) {
        // SAFETY: the control view is live for the segment's lifetime and the
        // key's mutex, or the local exclusion for an unnamed segment, is held.
        let header = unsafe { &mut *(segment.control as *mut ShmHeader) };
        header.nattch += 1;
        header.atime = now_seconds();
        header.lpid = own_pid();
    }
    view
}

/// Acquires the mutex guarding a segment's header.
///
/// An `IPC_PRIVATE` segment has no name and therefore no named mutex; it is also
/// unreachable from any other process, so the only writers are this process's own
/// threads and the state lock already excludes them from racing here.
fn shm_lock(segment: &Segment) -> Result<Option<CrossProcessLock>, i32> {
    if segment.key == IPC_PRIVATE {
        return CrossProcessLock::acquire(Some(&format!(
            "Local\\kinakaze.shm.private.{}.{}",
            segment.namespace, segment.private_lock
        )))
        .map(Some);
    }
    CrossProcessLock::acquire(Some(&object_name_in(
        "shmlock",
        segment.key,
        segment.namespace,
    )))
    .map(Some)
}

/// `shmdt`: detaches a segment previously returned by `shmat`.
///
/// Works on a segment that `IPC_RMID` already removed, which is required: the
/// removal only takes effect once the last attachment goes, so the detach that
/// completes it must still be accepted.
///
/// # Safety
///
/// `shmaddr` must be an address a previous `shmat` returned and that has not
/// already been detached.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_shmdt(shmaddr: *const c_void) -> c_int {
    let Ok(mut state) = state().lock() else {
        return fail(EIO);
    };
    // An address that was never attached is EINVAL, not a silent success: a
    // caller detaching the wrong pointer has a bug and must hear about it.
    let Some(_mapping) = kinakaze_runtime::begin_fork_mapping_transaction() else {
        return fail(EIO);
    };
    let Some(attachment) = state.attachments.remove(&(shmaddr as usize)) else {
        return fail(EINVAL);
    };
    let segment = attachment.segment;
    drop(state);

    // SAFETY: the address came out of the attachment table, so it is the base of
    // a view this module mapped and has not yet unmapped.
    if unsafe { UnmapViewOfFile(shmaddr) } == 0 {
        return fail(last_errno());
    }
    kinakaze_runtime::unregister_fork_mapping(shmaddr as usize);

    // The segment record is gone if IPC_RMID removed it, in which case there is
    // no header left to update; the detach itself still succeeded.
    if let Ok(_lock) = shm_lock(&segment) {
        // SAFETY: the control view is live while the Arc is held.
        let header = unsafe { &mut *(segment.control as *mut ShmHeader) };
        header.nattch = header.nattch.saturating_sub(1);
        header.dtime = now_seconds();
        header.lpid = own_pid();
    }
    0
}

/// `shmctl`: queries, alters or removes a segment.
///
/// `IPC_STAT` is answered from the shared header, so `shm_segsz` and
/// `shm_nattch` are true across processes rather than describing only this one.
/// The four id fields report 0 because this layer runs as uid 0 and has no second
/// identity to name; see `userdb`'s module header.
///
/// `IPC_SET` accepts a new mode and records it, and that is all it can do:
/// Windows decides access from the section object's DACL, which was fixed at
/// creation, so the recorded mode is reported back by `IPC_STAT` but is not
/// enforced. A caller tightening a mode to exclude another process will not get
/// that effect.
///
/// `IPC_RMID` closes this process's handle and control view and withdraws the id.
/// Attachments already mapped stay valid until they are detached, matching Linux.
/// The section — and with it the Windows name — disappears once no process holds
/// either a handle or a view.
///
/// # Safety
///
/// For `IPC_STAT` and `IPC_SET`, `buffer` must point at a `struct shmid_ds`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_shmctl(
    id: c_int,
    command: c_int,
    buffer: *mut ShmidDs,
) -> c_int {
    let segment = match find_segment(id) {
        Ok(segment) => segment,
        Err(error) => return fail(error),
    };

    match command {
        IPC_STAT => {
            if buffer.is_null() {
                return fail(EFAULT);
            }
            let Ok(_lock) = shm_lock(&segment) else {
                return fail(EIO);
            };
            // SAFETY: the control view is live while the Arc is held.
            let header = unsafe { &*(segment.control as *const ShmHeader) };
            let stat = ShmidDs {
                shm_perm: IpcPerm {
                    key: header.key,
                    mode: (header.mode & 0o777) as u16,
                    ..IpcPerm::default()
                },
                shm_segsz: header.segsz as usize,
                shm_atime: header.atime,
                shm_dtime: header.dtime,
                shm_ctime: header.ctime,
                shm_cpid: header.cpid,
                shm_lpid: header.lpid,
                shm_nattch: header.nattch,
                unused4: 0,
                unused5: 0,
            };
            // SAFETY: the caller guarantees a writable struct shmid_ds.
            unsafe { std::ptr::write(buffer, stat) };
            0
        }
        IPC_SET => {
            if buffer.is_null() {
                return fail(EFAULT);
            }
            // SAFETY: the caller guarantees a readable struct shmid_ds.
            let requested = unsafe { std::ptr::read(buffer) };
            let Ok(_lock) = shm_lock(&segment) else {
                return fail(EIO);
            };
            // SAFETY: as above.
            let header = unsafe { &mut *(segment.control as *mut ShmHeader) };
            header.mode = u32::from(requested.shm_perm.mode) & 0o777;
            header.ctime = now_seconds();
            0
        }
        IPC_RMID => {
            let Ok(_lock) = shm_lock(&segment) else {
                return fail(EIO);
            };
            // SAFETY: the control view is live while the Arc is held.
            let header = unsafe { &mut *(segment.control as *mut ShmHeader) };
            header.removed = 1;
            header.ctime = now_seconds();
            drop(_lock);

            let Ok(mut state) = state().lock() else {
                return fail(EIO);
            };
            state.segments.remove(&id);
            state.removed.insert(id);
            0
        }
        _ => fail(EINVAL),
    }
}
// ---------------------------------------------------------------------------
// Semaphores.
// ---------------------------------------------------------------------------

/// `semget`: creates or opens a semaphore set of `nsems` semaphores.
///
/// The set is a section holding a [`SemHeader`], a named mutex serializing every
/// operation on it, and a named event blocked waiters re-test on. All three names
/// derive from the key, so two processes reach one set.
///
/// `nsems` is capped at [`SEMMSL`] because the shared header is a fixed size;
/// Linux's own default limit is the same number, so a request this rejects would
/// have been rejected by a stock Linux kernel too.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_semget(key: Key, nsems: c_int, flags: c_int) -> c_int {
    let creating = flags & IPC_CREAT != 0;
    if nsems < 0 || nsems as usize > SEMMSL {
        return fail(EINVAL);
    }
    // Opening an existing set may pass 0 to mean "however many there are"; a
    // create must say how many it wants.
    if nsems == 0 && creating {
        return fail(EINVAL);
    }
    let nsems = nsems as usize;

    let name = (key != IPC_PRIVATE).then(|| object_name("sem", key));
    let _lock = match name.as_deref() {
        Some(_) => match CrossProcessLock::acquire(Some(&object_name("semlock", key))) {
            Ok(lock) => Some(lock),
            Err(error) => return fail(error),
        },
        None => None,
    };

    let (mapping, existed) = if creating || key == IPC_PRIVATE {
        // Sized to the whole control granule rather than to the header, because
        // that is what `map_control` maps; a section of exactly `SemHeader` bytes
        // rounds up only to a page and the larger view would be refused.
        match create_section(name.as_deref(), control_bytes() as u64) {
            Ok(pair) => pair,
            Err(error) => return fail(error),
        }
    } else {
        let wide_name = wide(name.as_deref().unwrap_or_default());
        // SAFETY: `wide_name` is a null-terminated wide string.
        let mapping =
            unsafe { OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, 0, wide_name.as_ptr()) };
        if mapping.is_null() {
            // SAFETY: GetLastError has no preconditions.
            let error = unsafe { GetLastError() };
            return fail(if error == ERROR_FILE_NOT_FOUND {
                ENOENT
            } else {
                errno_from_win32(error)
            });
        }
        (mapping, true)
    };

    if existed && creating && flags & IPC_EXCL != 0 {
        // SAFETY: owned here and not stored.
        unsafe { CloseHandle(mapping) };
        return fail(EEXIST);
    }

    let control = match map_control(mapping) {
        Ok(view) => view,
        Err(error) => {
            // SAFETY: owned here and not stored.
            unsafe { CloseHandle(mapping) };
            return fail(error);
        }
    };

    let count = match initialize_sem_header(control, existed, key, nsems, flags) {
        Ok(count) => count,
        Err(error) => {
            release_section(mapping, control);
            return fail(error);
        }
    };

    match open_set_objects(key) {
        Ok((lock, wake)) => {
            match publish_set(mapping, control, lock, wake, count) {
                Ok(id) => id,
                Err(error) => {
                    // SAFETY: neither handle has been published.
                    unsafe {
                        CloseHandle(lock);
                        CloseHandle(wake);
                    }
                    release_section(mapping, control);
                    fail(error)
                }
            }
        }
        Err(error) => {
            release_section(mapping, control);
            fail(error)
        }
    }
}

/// Opens the mutex and event a set needs, both named from the key.
///
/// An `IPC_PRIVATE` set gets unnamed objects: no other process can reach the set,
/// so nothing needs to find them by name, and the mutex still serializes this
/// process's own threads.
fn open_set_objects(key: Key) -> Result<(*mut c_void, *mut c_void), i32> {
    let (lock_name, wake_name) = if key == IPC_PRIVATE {
        (None, None)
    } else {
        (
            Some(wide(&object_name("semop", key))),
            Some(wide(&object_name("semwake", key))),
        )
    };
    let lock_ptr = lock_name
        .as_ref()
        .map_or(std::ptr::null(), |units| units.as_ptr());
    let wake_ptr = wake_name
        .as_ref()
        .map_or(std::ptr::null(), |units| units.as_ptr());

    // SAFETY: null attributes request the default descriptor and each name is
    // null or a null-terminated wide string.
    let lock = unsafe { CreateMutexW(std::ptr::null(), 0, lock_ptr) };
    if lock.is_null() {
        return Err(last_errno());
    }
    // Auto-reset and initially clear: each completed operation releases exactly
    // one waiter, which then re-tests its own condition under the mutex.
    // SAFETY: as above.
    let wake = unsafe { CreateEventW(std::ptr::null(), 0, 0, wake_ptr) };
    if wake.is_null() {
        let error = last_errno();
        // SAFETY: created just above and not stored.
        unsafe { CloseHandle(lock) };
        return Err(error);
    }
    Ok((lock, wake))
}

/// Initializes a fresh set header or validates an existing one.
fn initialize_sem_header(
    control: *mut c_void,
    existed: bool,
    key: Key,
    nsems: usize,
    flags: c_int,
) -> Result<usize, i32> {
    // SAFETY: `control` maps at least one granule, which exceeds a SemHeader, and
    // the key's mutex excludes every other writer.
    let header = unsafe { &mut *control.cast::<SemHeader>() };

    if !existed || header.magic != SEM_MAGIC {
        let now = now_seconds();
        header.magic = SEM_MAGIC;
        header.otime = 0;
        header.ctime = now;
        header.nsems = nsems as u32;
        header.key = key;
        header.mode = (flags & 0o777) as u32;
        header.removed = 0;
        // A new set's semaphores are all zero on Linux, which the zero-filled
        // section already provides; they are written anyway because a
        // re-initialized section may hold a dead creator's values.
        header.values = [0; SEMMSL];
        header.last_pid = [0; SEMMSL];
        header.undo = [UndoEntry {
            pid: 0,
            sem_num: 0,
            started: 0,
            adjust: 0,
            pad: 0,
        }; UNDO_SLOTS];
        return Ok(nsems);
    }

    if header.removed != 0 {
        return Err(EIDRM);
    }
    let existing = header.nsems as usize;
    // Linux reports EINVAL when the caller names more semaphores than the set
    // holds; a silent clamp would let it index past the end.
    if nsems > existing {
        return Err(EINVAL);
    }
    Ok(existing)
}

/// Records a set in the state table and returns its id.
fn publish_set(
    mapping: *mut c_void,
    control: *mut c_void,
    lock: *mut c_void,
    wake: *mut c_void,
    nsems: usize,
) -> Result<c_int, i32> {
    let mut state = state().lock().map_err(|_| EIO)?;
    let id = state.next_id;
    state.next_id += 1;
    state.sets.insert(
        id,
        Arc::new(SemSet {
            namespace: ipc_namespace(),
            mapping: mapping as usize,
            control: control as usize,
            lock: lock as usize,
            wake: wake as usize,
            nsems,
        }),
    );
    Ok(id)
}

/// Looks up a live set, distinguishing "removed" from "never existed".
fn find_set(id: c_int) -> Result<Arc<SemSet>, i32> {
    let state = state().lock().map_err(|_| EIO)?;
    if let Some(set) = state
        .sets
        .get(&id)
        .filter(|s| s.namespace == ipc_namespace())
    {
        return Ok(Arc::clone(set));
    }
    Err(if state.removed.contains(&id) {
        EIDRM
    } else {
        EINVAL
    })
}
/// This process's creation time, which pins its identity against id reuse.
fn own_start_time() -> u64 {
    static STARTED: OnceLock<u64> = OnceLock::new();
    *STARTED.get_or_init(|| {
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
        if ok == 0 { 0 } else { creation.ticks() }
    })
}

/// Whether the process that created an undo entry is still running.
///
/// The creation time is compared as well as the id, because Windows reuses
/// process ids: a new process holding a dead one's id would otherwise look alive
/// and its predecessor's adjustments would never be reversed. A zero recorded
/// time means the creation time could not be read when the entry was made, in
/// which case the id alone has to serve and a reused id makes this report "alive"
/// — the conservative direction, since a missed reclaim leaks a count whereas a
/// wrong reclaim corrupts a live process's semaphore.
fn process_is_alive(pid: u32, started: u64) -> bool {
    // SAFETY: the access mask is the documented minimum for GetProcessTimes and
    // the id is only read.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        // The process is gone, or is one this token may not open at all. The
        // latter cannot happen for a process that shares our session and our
        // user, which is the only kind that can reach a `Local\` object.
        return false;
    }
    let mut creation = FileTime::default();
    let mut exit = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    // SAFETY: the handle was just opened and all four out-parameters are writable
    // locals.
    let ok = unsafe {
        GetProcessTimes(
            process,
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    };
    // SAFETY: opened just above and not stored.
    unsafe { CloseHandle(process) };

    if ok == 0 {
        // The handle opened, so something holds the id; without a creation time
        // there is no evidence it is a different process.
        return true;
    }
    started == 0 || creation.ticks() == started
}

/// Reverses and frees the undo entries of every process that has died.
///
/// This is what makes `SEM_UNDO` crash-safe without an exit hook. Linux reverses
/// a process's adjustments in `do_exit`, which this layer cannot hook — a process
/// killed with `TerminateProcess` runs no code at all. Instead the work is done
/// *lazily by whoever needs it*: the surviving process that would otherwise block
/// forever on a lock a dead process held is exactly the process that reclaims it.
///
/// The scan is deliberately not on the fast path. It costs one `OpenProcess` per
/// occupied slot, so it runs only when an operation is about to block or is about
/// to report `EAGAIN` — the two cases where a dead holder is the plausible cause
/// and where the caller is already paying for a wait.
///
/// # Safety
///
/// `header` must be a live set header, with the set's mutex held.
unsafe fn reclaim_dead_undo(header: &mut SemHeader) -> bool {
    let mut reclaimed = false;
    for index in 0..UNDO_SLOTS {
        let entry = header.undo[index];
        if entry.pid == 0 {
            continue;
        }
        if process_is_alive(entry.pid, entry.started) {
            continue;
        }
        let slot = entry.sem_num as usize;
        if slot < SEMMSL {
            // Clamped rather than rejected: the reversal has to happen even if the
            // arithmetic would leave the range, and Linux clamps here too.
            header.values[slot] = (header.values[slot] + entry.adjust).clamp(0, SEMVMX);
        }
        header.undo[index].pid = 0;
        header.undo[index].adjust = 0;
        reclaimed = true;
    }
    reclaimed
}

/// CLONE_SYSVSEM detaches the caller's adjustment list, applying its debts.
/// Hosted processes do not share this list with separately created processes.
pub(crate) fn unshare_undo() -> Result<(), i32> {
    let sets: Vec<_> = state()
        .lock()
        .map_err(|_| EIO)?
        .sets
        .values()
        .cloned()
        .collect();
    let pid = own_host_pid();
    for set in sets {
        let _guard = CrossProcessLock::acquire_handle(set.lock as *mut c_void)?;
        let header = unsafe { &mut *(set.control as *mut SemHeader) };
        let mut changed = false;
        for index in 0..UNDO_SLOTS {
            let entry = header.undo[index];
            if entry.pid != pid {
                continue;
            }
            if (entry.sem_num as usize) < SEMMSL {
                let value = &mut header.values[entry.sem_num as usize];
                *value = (*value + entry.adjust).clamp(0, SEMVMX);
            }
            header.undo[index].pid = 0;
            header.undo[index].adjust = 0;
            changed = true;
        }
        if changed {
            unsafe { SetEvent(set.wake as *mut c_void) };
        }
    }
    Ok(())
}

/// Adds `delta` to the undo debt this process owes semaphore `sem_num`.
///
/// Returns `ENOSPC` when the table is full, which is what Linux reports when it
/// cannot allocate an undo structure. Failing here rather than proceeding is
/// required: an operation applied without its undo recorded is precisely the leak
/// `SEM_UNDO` exists to prevent.
fn record_undo(header: &mut SemHeader, sem_num: u32, delta: i32) -> Result<(), i32> {
    // SEM_UNDO is internal ownership state: Windows liveness checks must use
    // the real pid, never the namespace pid exposed through `GETPID`.
    let pid = own_host_pid();
    let started = own_start_time();
    if let Some(entry) = header
        .undo
        .iter_mut()
        .find(|entry| entry.pid == pid && entry.sem_num == sem_num)
    {
        entry.adjust = entry.adjust.saturating_add(delta);
        // An adjustment that cancels out is dropped, so a lock taken and released
        // repeatedly does not hold a slot forever.
        if entry.adjust == 0 {
            entry.pid = 0;
        }
        return Ok(());
    }
    if delta == 0 {
        return Ok(());
    }
    let slot = header
        .undo
        .iter_mut()
        .find(|entry| entry.pid == 0)
        .ok_or(ENOSPC)?;
    *slot = UndoEntry {
        pid,
        sem_num,
        started,
        adjust: delta,
        pad: 0,
    };
    Ok(())
}

/// What one pass over the operation array concluded.
enum Attempt {
    /// Every operation applied. The flag says whether any value changed, which
    /// decides whether a waiter needs waking.
    Applied {
        changed: bool,
    },
    /// Operation `index` cannot proceed yet.
    Blocked {
        index: usize,
    },
    Failed(i32),
}

/// Applies the whole array, or none of it.
///
/// This is the function `semop`'s contract rests on. Every operation is tested
/// against a *copy* of the values, so an array that cannot complete leaves the set
/// untouched; only once all of them are known to succeed is the copy committed.
/// A loop of independent waits could not do this, which is the reason `semop`
/// takes an array at all.
///
/// # Safety
///
/// `header` must be a live set header, with the set's mutex held.
unsafe fn try_apply(header: &mut SemHeader, ops: &[Sembuf]) -> Attempt {
    let mut candidate = header.values;
    let mut changed = false;

    for (index, op) in ops.iter().enumerate() {
        let slot = op.sem_num as usize;
        let current = candidate[slot];
        let delta = i32::from(op.sem_op);

        if delta == 0 {
            // Waiting for zero. No Windows primitive expresses this; it works here
            // because the whole set is inspected under a lock rather than waited on
            // through a kernel object.
            if current != 0 {
                return Attempt::Blocked { index };
            }
            continue;
        }
        let next = current + delta;
        if next > SEMVMX {
            // Linux reports ERANGE for an increment that would exceed SEMVMX, and
            // does so without applying any of the array.
            return Attempt::Failed(ERANGE);
        }
        if next < 0 {
            return Attempt::Blocked { index };
        }
        candidate[slot] = next;
        changed = true;
    }

    // Every operation is feasible, so the undo debts can be recorded. This is
    // still ahead of the commit: an ENOSPC here must leave the set untouched.
    let mut recorded: Vec<(u32, i32)> = Vec::new();
    for op in ops {
        if op.sem_flg & SEM_UNDO as i16 == 0 || op.sem_op == 0 {
            continue;
        }
        // The debt is the reverse of what is being applied.
        let delta = -i32::from(op.sem_op);
        if let Err(error) = record_undo(header, u32::from(op.sem_num), delta) {
            // Unwind the debts already written so the failure changes nothing.
            for (sem_num, applied) in recorded {
                let _ = record_undo(header, sem_num, -applied);
            }
            return Attempt::Failed(error);
        }
        recorded.push((u32::from(op.sem_num), delta));
    }

    let pid = own_pid();
    header.values = candidate;
    for op in ops {
        header.last_pid[op.sem_num as usize] = pid;
    }
    header.otime = now_seconds();
    Attempt::Applied { changed }
}
/// How long a blocked `semop` waits before re-testing regardless of the event.
///
/// The event makes a wakeup prompt in the normal case. The timeout bounds the
/// abnormal one: a process that died holding a lock signals nothing, so without a
/// ceiling a waiter would sleep forever behind a debt that
/// [`reclaim_dead_undo`] is ready to settle. 20 ms is far below any timescale a
/// caller of a blocking `semop` cares about and costs 50 wakeups a second while
/// genuinely contended.
const RETRY_MILLISECONDS: u32 = 20;

/// `semop`: applies an array of operations atomically.
///
/// Atomicity is real, and it is the whole point. All the operations are tested
/// against a copy of the set under the set's named mutex, and are committed only
/// if every one of them can proceed — so a caller that takes two semaphores in one
/// call can never end up holding one of them. Blocking on zero
/// (`sem_op == 0`) works for the same reason: the condition is evaluated under the
/// lock rather than waited on through a kernel object, and Windows has no
/// primitive for it.
///
/// `SEM_UNDO` is implemented, including after a crash. See [`reclaim_dead_undo`]
/// for how a dead process's adjustments are reversed without an exit hook.
///
/// A blocked call wakes on an event the next completed operation signals, and
/// re-tests every [`RETRY_MILLISECONDS`] regardless so a dead lock-holder cannot
/// wedge it. The two divergences from Linux: the wait is not interruptible by a
/// signal, so it never reports `EINTR`, and a wakeup can be up to that timeout
/// late in the pathological case.
///
/// # Safety
///
/// `ops` must name at least `count` readable `struct sembuf`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_semop(
    id: c_int,
    ops: *const Sembuf,
    count: usize,
) -> c_int {
    if count == 0 {
        // Linux accepts an empty array as a no-op that still validates the id.
        return match find_set(id) {
            Ok(_) => 0,
            Err(error) => fail(error),
        };
    }
    if count > SEMOPM {
        return fail(EINVAL);
    }
    if ops.is_null() {
        return fail(EFAULT);
    }
    let set = match find_set(id) {
        Ok(set) => set,
        Err(error) => return fail(error),
    };
    // Copied out of guest memory once, so the array cannot change underneath the
    // feasibility test and the commit that follows it.
    // SAFETY: the caller guarantees `count` readable sembuf.
    let ops: Vec<Sembuf> = unsafe { std::slice::from_raw_parts(ops, count) }.to_vec();

    // Every index is validated before anything is locked, so a bad array is
    // rejected without disturbing the set.
    if ops.iter().any(|op| op.sem_num as usize >= set.nsems) {
        return fail(EFBIG);
    }

    loop {
        let Ok(guard) = CrossProcessLock::acquire_handle(set.lock as *mut c_void) else {
            return fail(EIO);
        };
        // SAFETY: the control view is live while the Arc is held, and the set's
        // mutex is held for the whole of this block.
        let header = unsafe { &mut *(set.control as *mut SemHeader) };
        if header.removed != 0 {
            return fail(EIDRM);
        }

        // SAFETY: as above.
        match unsafe { try_apply(header, &ops) } {
            Attempt::Applied { changed } => {
                drop(guard);
                if changed {
                    // SAFETY: the event handle is live while the Arc is held.
                    unsafe { SetEvent(set.wake as *mut c_void) };
                }
                return 0;
            }
            Attempt::Failed(error) => {
                drop(guard);
                return fail(error);
            }
            Attempt::Blocked { index } => {
                // A dead process's undo debt is the one blocker a waiter can clear
                // itself, so it is settled before deciding to wait at all.
                // SAFETY: as above.
                if unsafe { reclaim_dead_undo(header) } {
                    drop(guard);
                    continue;
                }
                // IPC_NOWAIT is per-operation: it is the operation that would have
                // blocked whose flag decides, not the first in the array.
                if ops[index].sem_flg & IPC_NOWAIT as i16 != 0 {
                    drop(guard);
                    return fail(EAGAIN);
                }
                // Released before waiting, or no other process could ever make
                // progress and the wait would be a deadlock.
                drop(guard);
                // SAFETY: the event handle is live while the Arc is held.
                match unsafe { WaitForSingleObject(set.wake as *mut c_void, RETRY_MILLISECONDS) } {
                    WAIT_OBJECT_0 | WAIT_TIMEOUT => {}
                    _ => return fail(EIO),
                }
            }
        }
    }
}

/// `EFBIG`, reported when `sem_num` names a semaphore outside the set.
///
/// Linux uses this rather than `EINVAL` for an out-of-range `sem_num`, and the
/// distinction is load-bearing: a caller that grew its set and got the size wrong
/// sees a different error from one that passed a stale identifier.
const EFBIG: i32 = 27;
/// Drops every process's undo debt against one semaphore.
///
/// `SETVAL` and `SETALL` do this on Linux, and they must: a debt is a promise to
/// restore a value that a direct assignment has just invalidated, so honouring it
/// afterwards would undo the assignment instead.
fn clear_undo(header: &mut SemHeader, sem_num: Option<u32>) {
    for entry in &mut header.undo {
        if entry.pid != 0 && sem_num.is_none_or(|wanted| entry.sem_num == wanted) {
            entry.pid = 0;
            entry.adjust = 0;
        }
    }
}

/// `semctl`: queries, alters or removes a semaphore set.
///
/// The fourth argument is variadic on Linux — a `union semun` passed by value.
/// It needs none of [`crate::variadic`]'s machinery: the System V AMD64 ABI
/// passes an 8-byte union in the same register a fixed fourth argument would use,
/// so declaring it as a `usize` receives exactly what a variadic caller sent. The
/// register holds an `int` for `SETVAL`, a pointer for the array and stat
/// commands, and is ignored for the rest — which is why the commands that do not
/// take an argument never read it, since the caller left it undefined.
///
/// Implemented: `IPC_STAT`, `IPC_SET`, `IPC_RMID`, `GETVAL`, `SETVAL`, `GETALL`,
/// `SETALL`, `GETPID`.
///
/// Refused with `ENOSYS`: `GETNCNT` and `GETZCNT`, the counts of processes
/// blocked on a semaphore. Reporting these honestly would mean registering every
/// waiter in the shared header and reclaiming a dead waiter's registration the way
/// [`reclaim_dead_undo`] reclaims a debt. They are diagnostics that no correctness
/// argument rests on, and a fabricated count is worse than none: a monitoring tool
/// would report a queue that does not exist.
///
/// # Safety
///
/// `argument` must match what `command` requires: a `struct semid_ds *` for
/// `IPC_STAT` and `IPC_SET`, an `unsigned short *` for `GETALL` and `SETALL`, an
/// `int` for `SETVAL`, and nothing at all for the others.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_semctl(
    id: c_int,
    sem_num: c_int,
    command: c_int,
    argument: usize,
) -> c_int {
    let set = match find_set(id) {
        Ok(set) => set,
        Err(error) => return fail(error),
    };
    // The commands that name one semaphore must name one that exists.
    if matches!(command, GETVAL | SETVAL | GETPID | GETNCNT | GETZCNT)
        && (sem_num < 0 || sem_num as usize >= set.nsems)
    {
        return fail(EINVAL);
    }

    let Ok(guard) = CrossProcessLock::acquire_handle(set.lock as *mut c_void) else {
        return fail(EIO);
    };
    // SAFETY: the control view is live while the Arc is held and the set's mutex
    // is held for the whole of this block.
    let header = unsafe { &mut *(set.control as *mut SemHeader) };
    // Every command but IPC_RMID needs the set to still be there. IPC_RMID on an
    // already-removed set is EIDRM too, since the id was withdrawn from this
    // process's table when it happened.
    if header.removed != 0 {
        return fail(EIDRM);
    }
    let slot = sem_num as usize;

    match command {
        GETVAL => header.values[slot],
        GETPID => header.last_pid[slot],
        SETVAL => {
            // The union carries an `int`, so only the low half is the value.
            let value = argument as u32 as i32;
            if !(0..=SEMVMX).contains(&value) {
                return fail(ERANGE);
            }
            header.values[slot] = value;
            clear_undo(header, Some(sem_num as u32));
            header.ctime = now_seconds();
            drop(guard);
            // SAFETY: the event handle is live while the Arc is held.
            unsafe { SetEvent(set.wake as *mut c_void) };
            0
        }
        GETALL => {
            let values = argument as *mut u16;
            if values.is_null() {
                return fail(EFAULT);
            }
            for index in 0..set.nsems {
                // SAFETY: the caller guarantees `nsems` writable unsigned shorts,
                // which is the array size the set's own `nsems` describes.
                unsafe { *values.add(index) = header.values[index] as u16 };
            }
            0
        }
        SETALL => {
            let values = argument as *const u16;
            if values.is_null() {
                return fail(EFAULT);
            }
            // Read and validated in full before anything is written, so a value out
            // of range leaves the whole set as it was.
            let mut requested = [0i32; SEMMSL];
            for (index, slot) in requested.iter_mut().enumerate().take(set.nsems) {
                // SAFETY: the caller guarantees `nsems` readable unsigned shorts.
                let value = i32::from(unsafe { *values.add(index) });
                if value > SEMVMX {
                    return fail(ERANGE);
                }
                *slot = value;
            }
            header.values[..set.nsems].copy_from_slice(&requested[..set.nsems]);
            clear_undo(header, None);
            header.ctime = now_seconds();
            drop(guard);
            // SAFETY: the event handle is live while the Arc is held.
            unsafe { SetEvent(set.wake as *mut c_void) };
            0
        }
        IPC_STAT => {
            let buffer = argument as *mut SemidDs;
            if buffer.is_null() {
                return fail(EFAULT);
            }
            let stat = SemidDs {
                sem_perm: IpcPerm {
                    key: header.key,
                    mode: (header.mode & 0o777) as u16,
                    ..IpcPerm::default()
                },
                sem_otime: header.otime,
                sem_ctime: header.ctime,
                sem_nsems: u64::from(header.nsems),
                unused3: 0,
                unused4: 0,
            };
            // SAFETY: the caller guarantees a writable struct semid_ds.
            unsafe { std::ptr::write(buffer, stat) };
            0
        }
        IPC_SET => {
            let buffer = argument as *const SemidDs;
            if buffer.is_null() {
                return fail(EFAULT);
            }
            // SAFETY: the caller guarantees a readable struct semid_ds.
            let requested = unsafe { std::ptr::read(buffer) };
            // Recorded but not enforced, for the reason `shmctl`'s IPC_SET gives:
            // the section's DACL is what Windows checks.
            header.mode = u32::from(requested.sem_perm.mode) & 0o777;
            header.ctime = now_seconds();
            0
        }
        IPC_RMID => {
            header.removed = 1;
            header.ctime = now_seconds();
            drop(guard);
            // Waiters must be released so they can see the removal and report
            // EIDRM rather than sleeping on a set that no longer exists.
            // SAFETY: the event handle is live while the Arc is held.
            unsafe { SetEvent(set.wake as *mut c_void) };

            let Ok(mut state) = state().lock() else {
                return fail(EIO);
            };
            state.sets.remove(&id);
            state.removed.insert(id);
            0
        }
        GETNCNT | GETZCNT => fail(ENOSYS),
        _ => fail(EINVAL),
    }
}

/// `ftok`: derives a key from a path and a one-byte project id.
///
/// Without this a caller cannot produce a key at all, so none of the rest of this
/// module would be reachable. The derivation is glibc's: the low 16 bits of the
/// inode, the low 8 bits of the device, and the project id in the top byte. It is
/// reproduced exactly rather than improved on, because two programs that agree on
/// a path must agree on the key, and one of them may be linked against real glibc
/// on the other side of a shared filesystem.
///
/// The inode and device come from this layer's own `stat`, which synthesizes them
/// from the host file. They are stable for a given file, which is the property
/// `ftok` needs; they are not the numbers a Linux filesystem would report, so a
/// key derived here will not match one derived on Linux for the same file. Nothing
/// can make it, and no caller can tell without comparing across kernels.
///
/// # Safety
///
/// `path` must be null or a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ftok(path: *const c_char, project: c_int) -> Key {
    if path.is_null() {
        return fail(EFAULT);
    }
    // SAFETY: the caller guarantees a null-terminated string.
    let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() else {
        // A path that is not valid UTF-8 cannot be resolved by this layer's VFS.
        return fail(EINVAL);
    };
    let stat = match kinakaze_vfs::fs::stat(path) {
        Ok(stat) => stat,
        Err(error) => return fail(error),
    };
    let inode = (stat.st_ino & 0xffff) as u32;
    let device = (stat.st_dev & 0xff) as u32;
    let project = (project as u32 & 0xff) << 24;
    (project | (device << 16) | inode) as Key
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    /// A key nothing else will use, derived from this process and a caller tag.
    ///
    /// Named objects live in a session-wide namespace, so two `cargo test`
    /// processes running at once would otherwise collide on a fixed key and see
    /// each other's segments. Mixing in the pid makes each run's keys its own; the
    /// tag separates the tests within a run, which matters because they execute in
    /// parallel threads of one process.
    fn unique_key(tag: u32) -> Key {
        ((own_pid() as u32) << 8 | (tag & 0xff)) as Key
    }

    #[test]
    fn struct_layouts_match_the_linux_x86_64_abi() {
        // Guest code passes these structures across the boundary and reads the
        // fields back by offset, so a layout change is a silent ABI break.
        assert_eq!(size_of::<IpcPerm>(), 48);
        assert_eq!(offset_of!(IpcPerm, key), 0);
        assert_eq!(offset_of!(IpcPerm, uid), 4);
        assert_eq!(offset_of!(IpcPerm, gid), 8);
        assert_eq!(offset_of!(IpcPerm, cuid), 12);
        assert_eq!(offset_of!(IpcPerm, cgid), 16);
        assert_eq!(offset_of!(IpcPerm, mode), 20);
        assert_eq!(offset_of!(IpcPerm, seq), 24);

        assert_eq!(size_of::<ShmidDs>(), 112);
        assert_eq!(offset_of!(ShmidDs, shm_perm), 0);
        assert_eq!(offset_of!(ShmidDs, shm_segsz), 48);
        assert_eq!(offset_of!(ShmidDs, shm_atime), 56);
        assert_eq!(offset_of!(ShmidDs, shm_dtime), 64);
        assert_eq!(offset_of!(ShmidDs, shm_ctime), 72);
        assert_eq!(offset_of!(ShmidDs, shm_cpid), 80);
        assert_eq!(offset_of!(ShmidDs, shm_lpid), 84);
        assert_eq!(offset_of!(ShmidDs, shm_nattch), 88);

        assert_eq!(size_of::<SemidDs>(), 88);
        assert_eq!(offset_of!(SemidDs, sem_otime), 48);
        assert_eq!(offset_of!(SemidDs, sem_ctime), 56);
        assert_eq!(offset_of!(SemidDs, sem_nsems), 64);

        // Six bytes with two-byte alignment, so `sops[2]` lands at 12. Padding
        // here would shift every operation after the first.
        assert_eq!(size_of::<Sembuf>(), 6);
        assert_eq!(align_of::<Sembuf>(), 2);
        assert_eq!(offset_of!(Sembuf, sem_num), 0);
        assert_eq!(offset_of!(Sembuf, sem_op), 2);
        assert_eq!(offset_of!(Sembuf, sem_flg), 4);
    }

    #[test]
    fn the_key_to_name_mapping_is_deterministic_and_injective() {
        // Two processes must derive the same name from the same key, or they get
        // different segments and share nothing.
        assert_eq!(object_name("shm", 42), object_name("shm", 42));
        assert_eq!(
            object_name("shm", 42),
            format!(
                "Local\\kinakaze.shm.ns{}.{}.0000002a",
                kinakaze_runtime::authority::domain_id(),
                ipc_namespace()
            )
        );

        // Distinct keys must give distinct names, including across the sign
        // boundary: -1 and 0xffffffff are the same bit pattern and must not become
        // two different names, while -1 and 1 must not become the same one.
        assert_ne!(object_name("shm", 42), object_name("shm", 43));
        assert_ne!(object_name("shm", -1), object_name("shm", 1));
        assert_eq!(
            object_name("shm", -1),
            format!(
                "Local\\kinakaze.shm.ns{}.{}.ffffffff",
                kinakaze_runtime::authority::domain_id(),
                ipc_namespace()
            )
        );
        // The name is fixed-width, so no two keys can produce one name by one's
        // digits running into the next field.
        for name in [
            object_name("shm", 0),
            object_name("shm", i32::MIN),
            object_name("shm", i32::MAX),
        ] {
            assert_eq!(
                name.len(),
                format!(
                    "Local\\kinakaze.shm.ns{}.{}.",
                    kinakaze_runtime::authority::domain_id(),
                    ipc_namespace()
                )
                .len()
                    + 8
            );
        }

        // The kinds are separate namespaces, so a segment and a set may share a
        // key without colliding.
        assert_ne!(object_name("shm", 7), object_name("sem", 7));
        // Every name is in the per-session namespace the module header documents.
        assert!(object_name("shm", 7).starts_with("Local\\"));
    }

    #[test]
    fn a_shared_segment_round_trips_through_detach_and_reattach() {
        // IPC_PRIVATE: no name, so nothing to clean out of the session namespace
        // and no chance of colliding with a parallel test.
        let id = kinakaze_abi_shmget(IPC_PRIVATE, 4096, IPC_CREAT | 0o600);
        assert!(id > 0, "shmget failed with errno {}", kinakaze_tls::errno());

        // SAFETY: `id` names a live segment and a null address lets the kernel
        // choose where the view lands.
        let first = unsafe { kinakaze_abi_shmat(id, std::ptr::null(), 0) };
        assert_ne!(first, SHM_FAILED, "shmat failed: {}", kinakaze_tls::errno());

        let message = b"kinakaze shared memory";
        // SAFETY: the view is 4096 writable bytes and the message is far shorter.
        unsafe {
            std::ptr::copy_nonoverlapping(message.as_ptr(), first.cast::<u8>(), message.len());
        }

        // SAFETY: `first` is the address the matching shmat returned.
        assert_eq!(unsafe { kinakaze_abi_shmdt(first) }, 0);

        // The section is still alive because this process holds its handle, which
        // is what makes the data survive the detach.
        // SAFETY: as above.
        let second = unsafe { kinakaze_abi_shmat(id, std::ptr::null(), 0) };
        assert_ne!(second, SHM_FAILED);
        // SAFETY: the fresh view is at least as long as the message.
        let observed = unsafe { std::slice::from_raw_parts(second.cast::<u8>(), message.len()) };
        assert_eq!(observed, message, "the segment lost its contents");

        // SAFETY: `second` came from the shmat above.
        assert_eq!(unsafe { kinakaze_abi_shmdt(second) }, 0);
        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(
            unsafe { kinakaze_abi_shmctl(id, IPC_RMID, std::ptr::null_mut()) },
            0
        );
    }

    #[test]
    fn ipc_excl_reports_eexist_the_second_time() {
        let key = unique_key(1);
        let first = kinakaze_abi_shmget(key, 4096, IPC_CREAT | IPC_EXCL | 0o600);
        assert!(
            first > 0,
            "the first create failed: {}",
            kinakaze_tls::errno()
        );

        // This is how callers arbitrate ownership, so it has to be the host's own
        // "already existed" answer rather than this module's bookkeeping: the
        // creator could have been another process entirely.
        set_errno(0);
        let second = kinakaze_abi_shmget(key, 4096, IPC_CREAT | IPC_EXCL | 0o600);
        assert_eq!(second, -1, "the second create should have been refused");
        assert_eq!(kinakaze_tls::errno(), EEXIST);

        // Without IPC_EXCL the same key opens the existing segment instead.
        let opened = kinakaze_abi_shmget(key, 4096, IPC_CREAT | 0o600);
        assert!(opened > 0);
        // A different id for the same segment: ids are process-local indices, as
        // the module header states.
        assert_ne!(opened, first);

        // SAFETY: IPC_RMID reads no buffer. Both ids must be removed, since each
        // holds its own handle and the section outlives either one alone.
        unsafe {
            assert_eq!(
                kinakaze_abi_shmctl(opened, IPC_RMID, std::ptr::null_mut()),
                0
            );
            assert_eq!(
                kinakaze_abi_shmctl(first, IPC_RMID, std::ptr::null_mut()),
                0
            );
        }
    }

    #[test]
    fn shmctl_stat_reports_the_size_and_attachment_count() {
        let id = kinakaze_abi_shmget(IPC_PRIVATE, 8192, IPC_CREAT | 0o640);
        assert!(id > 0);

        let mut stat = ShmidDs::default();
        // SAFETY: `stat` is a writable local of the right type.
        assert_eq!(
            unsafe { kinakaze_abi_shmctl(id, IPC_STAT, &raw mut stat) },
            0
        );
        // The size the caller asked for, not the granule-rounded section size: a
        // caller that sized a loop from this must not run past its own segment.
        assert_eq!(stat.shm_segsz, 8192);
        assert_eq!(stat.shm_nattch, 0);
        assert_eq!(stat.shm_cpid, own_pid());
        assert_eq!(stat.shm_perm.mode, 0o640);
        assert!(stat.shm_ctime > 0, "ctime should be a real timestamp");

        // SAFETY: `id` is live and a null address lets the kernel choose.
        let view = unsafe { kinakaze_abi_shmat(id, std::ptr::null(), 0) };
        assert_ne!(view, SHM_FAILED);
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_shmctl(id, IPC_STAT, &raw mut stat) },
            0
        );
        assert_eq!(stat.shm_nattch, 1, "the attachment was not counted");
        assert!(stat.shm_atime > 0);

        // SAFETY: `view` came from the shmat above.
        assert_eq!(unsafe { kinakaze_abi_shmdt(view) }, 0);
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_shmctl(id, IPC_STAT, &raw mut stat) },
            0
        );
        assert_eq!(stat.shm_nattch, 0, "the detachment was not counted");

        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(
            unsafe { kinakaze_abi_shmctl(id, IPC_RMID, std::ptr::null_mut()) },
            0
        );
    }
    /// The environment variable carrying a key to the child half of the
    /// cross-process test.
    const CHILD_KEY: &str = "KINAKAZE_SHM_CHILD_KEY";

    /// What the child writes, for the parent to find in its own mapping.
    const CHILD_MESSAGE: &[u8] = b"written by another process";

    /// The child half of [`a_named_segment_is_shared_with_another_process`].
    ///
    /// Ignored by default because it is meaningless without the parent's key in
    /// the environment; the parent runs it explicitly.
    #[test]
    #[ignore = "spawned by a_named_segment_is_shared_with_another_process"]
    fn shm_child_writes_through_the_key() {
        let key: Key = std::env::var(CHILD_KEY)
            .expect("the parent must pass a key")
            .parse()
            .expect("the key must be an integer");

        // Deliberately without IPC_CREAT: the segment must already exist, so this
        // can only succeed by finding the parent's through the key alone. That is
        // the entire claim the name derivation makes.
        let id = kinakaze_abi_shmget(key, 4096, 0o600);
        assert!(
            id > 0,
            "the child could not find the segment: {}",
            kinakaze_tls::errno()
        );

        // SAFETY: `id` is live and a null address lets the kernel choose.
        let view = unsafe { kinakaze_abi_shmat(id, std::ptr::null(), 0) };
        assert_ne!(view, SHM_FAILED);

        // The attach count must already include the parent's attachment, which is
        // only possible if the header is genuinely shared.
        let mut stat = ShmidDs::default();
        // SAFETY: `stat` is a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_shmctl(id, IPC_STAT, &raw mut stat) },
            0
        );
        assert_eq!(stat.shm_segsz, 4096, "the child read the wrong size");
        assert_eq!(
            stat.shm_nattch, 2,
            "the parent's attachment was not visible"
        );

        // SAFETY: the view is 4096 bytes and the message is far shorter.
        unsafe {
            std::ptr::copy_nonoverlapping(
                CHILD_MESSAGE.as_ptr(),
                view.cast::<u8>(),
                CHILD_MESSAGE.len(),
            );
        }
        // SAFETY: `view` came from the shmat above.
        assert_eq!(unsafe { kinakaze_abi_shmdt(view) }, 0);
        // Deliberately no IPC_RMID: the parent still needs the segment.
    }

    #[test]
    fn a_named_segment_is_shared_with_another_process() {
        // The property the whole key namespace exists for, tested against a real
        // second process rather than assumed from the name derivation.
        let key = unique_key(3);
        let id = kinakaze_abi_shmget(key, 4096, IPC_CREAT | IPC_EXCL | 0o600);
        assert!(id > 0, "shmget failed: {}", kinakaze_tls::errno());
        // SAFETY: `id` is live and a null address lets the kernel choose.
        let view = unsafe { kinakaze_abi_shmat(id, std::ptr::null(), 0) };
        assert_ne!(view, SHM_FAILED);

        let child = std::process::Command::new(
            std::env::current_exe().expect("the test binary must be locatable"),
        )
        .args([
            "--exact",
            "sysvipc::tests::shm_child_writes_through_the_key",
            "--ignored",
        ])
        .env(CHILD_KEY, key.to_string())
        .output()
        .expect("failed to spawn the child half");
        assert!(
            child.status.success(),
            "the child failed:\n{}{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );

        // The bytes a different process wrote, visible here through nothing but the
        // shared key.
        // SAFETY: the view is 4096 bytes and the message is far shorter.
        let observed =
            unsafe { std::slice::from_raw_parts(view.cast::<u8>(), CHILD_MESSAGE.len()) };
        assert_eq!(observed, CHILD_MESSAGE, "the segment was not shared");

        // The child's detach was counted in the shared header too, so the count is
        // back to this process's single attachment.
        let mut stat = ShmidDs::default();
        // SAFETY: `stat` is a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_shmctl(id, IPC_STAT, &raw mut stat) },
            0
        );
        assert_eq!(stat.shm_nattch, 1, "the child's detach was not counted");
        // The child was the last process to touch it, which shm_lpid records.
        assert_ne!(stat.shm_lpid, 0);

        // SAFETY: `view` came from the shmat above.
        assert_eq!(unsafe { kinakaze_abi_shmdt(view) }, 0);
        // SAFETY: IPC_RMID reads no buffer. This releases the last handle, so the
        // section and its name are gone when the test returns.
        assert_eq!(
            unsafe { kinakaze_abi_shmctl(id, IPC_RMID, std::ptr::null_mut()) },
            0
        );
    }

    #[test]
    fn a_removed_segment_reports_eidrm_and_an_unknown_id_reports_einval() {
        let id = kinakaze_abi_shmget(IPC_PRIVATE, 4096, IPC_CREAT | 0o600);
        assert!(id > 0);
        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(
            unsafe { kinakaze_abi_shmctl(id, IPC_RMID, std::ptr::null_mut()) },
            0
        );

        // EIDRM, not EINVAL: the caller must be able to tell "it was removed" from
        // "you passed nonsense", because the first is a normal shutdown race.
        set_errno(0);
        // SAFETY: a removed id is still a well-formed argument.
        assert_eq!(
            unsafe { kinakaze_abi_shmat(id, std::ptr::null(), 0) },
            SHM_FAILED
        );
        assert_eq!(kinakaze_tls::errno(), EIDRM);

        set_errno(0);
        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(
            unsafe { kinakaze_abi_shmctl(id, IPC_RMID, std::ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EIDRM);

        // An id that was never handed out is EINVAL.
        set_errno(0);
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_shmat(999_999, std::ptr::null(), 0) },
            SHM_FAILED
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn shmget_without_ipc_creat_requires_an_existing_segment() {
        // A key nobody created: ENOENT, rather than a segment conjured up.
        set_errno(0);
        assert_eq!(kinakaze_abi_shmget(unique_key(2), 4096, 0o600), -1);
        assert_eq!(kinakaze_tls::errno(), ENOENT);

        // A create of nothing is rejected; Linux does the same.
        set_errno(0);
        assert_eq!(kinakaze_abi_shmget(IPC_PRIVATE, 0, IPC_CREAT | 0o600), -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn shmdt_of_an_address_that_was_never_attached_is_refused() {
        let mut byte = 0u8;
        set_errno(0);
        // SAFETY: the address is readable and simply is not an attachment.
        assert_eq!(unsafe { kinakaze_abi_shmdt((&raw mut byte).cast()) }, -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn shm_rdonly_maps_a_view_that_cannot_be_written() {
        let id = kinakaze_abi_shmget(IPC_PRIVATE, 4096, IPC_CREAT | 0o600);
        assert!(id > 0);
        // SAFETY: `id` is live and a null address lets the kernel choose.
        let view = unsafe { kinakaze_abi_shmat(id, std::ptr::null(), SHM_RDONLY) };
        assert_ne!(view, SHM_FAILED);

        // The protection is real, not merely recorded: VirtualQuery reports what
        // the kernel will enforce. Writing through the pointer would fault, so the
        // page protection is inspected instead of provoking the fault.
        let mut info = MemoryBasicInformation::default();
        // SAFETY: `view` is a live mapping and `info` is a writable local of the
        // size the call expects.
        let queried =
            unsafe { VirtualQuery(view, &raw mut info, size_of::<MemoryBasicInformation>()) };
        assert_ne!(queried, 0, "VirtualQuery failed");
        const PAGE_READONLY: u32 = 0x02;
        assert_eq!(info.protect, PAGE_READONLY, "SHM_RDONLY was not enforced");

        // SAFETY: `view` came from the shmat above.
        assert_eq!(unsafe { kinakaze_abi_shmdt(view) }, 0);
        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(
            unsafe { kinakaze_abi_shmctl(id, IPC_RMID, std::ptr::null_mut()) },
            0
        );
    }

    /// `MEMORY_BASIC_INFORMATION`, read only for its `protect` word.
    ///
    /// Spelled out rather than treated as an opaque buffer so the `VirtualQuery`
    /// signature matches the crate's other declaration of it.
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
        /// `VirtualQuery`, used only to confirm a read-only attach really is one.
        fn VirtualQuery(
            address: *const c_void,
            buffer: *mut MemoryBasicInformation,
            length: usize,
        ) -> usize;
    }
    /// Applies one operation, as the common single-semaphore case.
    fn one_op(id: c_int, sem_num: u16, sem_op: i16, sem_flg: i16) -> c_int {
        let ops = [Sembuf {
            sem_num,
            sem_op,
            sem_flg,
        }];
        // SAFETY: `ops` is one readable sembuf.
        unsafe { kinakaze_abi_semop(id, ops.as_ptr(), ops.len()) }
    }

    #[test]
    fn a_semaphore_counts_up_and_down() {
        let id = kinakaze_abi_semget(IPC_PRIVATE, 2, IPC_CREAT | 0o600);
        assert!(id > 0, "semget failed: {}", kinakaze_tls::errno());

        // A new set starts at zero on Linux, which callers rely on when they
        // initialize with SETVAL only if they created the set.
        // SAFETY: GETVAL reads no buffer.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, GETVAL, 0) }, 0);

        // Post, then take: the ordinary mutex idiom.
        assert_eq!(one_op(id, 0, 1, 0), 0);
        // SAFETY: as above.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, GETVAL, 0) }, 1);
        assert_eq!(one_op(id, 0, -1, 0), 0);
        // SAFETY: as above.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, GETVAL, 0) }, 0);

        // GETPID names the last process to touch it, which is truthful rather than
        // fabricated: it is this process.
        // SAFETY: as above.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, GETPID, 0) }, own_pid());

        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, IPC_RMID, 0) }, 0);
    }

    #[test]
    fn semop_applies_the_whole_array_or_none_of_it() {
        let id = kinakaze_abi_semget(IPC_PRIVATE, 3, IPC_CREAT | 0o600);
        assert!(id > 0);
        // Two of the three semaphores can be taken; the third cannot.
        for slot in [0, 1] {
            assert_eq!(one_op(id, slot, 1, 0), 0);
        }

        // This is the property that makes semop take an array. A loop of
        // independent waits would take semaphores 0 and 1 and then block, leaving
        // the caller holding two of the three locks it asked for — the classic
        // deadlock this interface exists to prevent.
        let ops = [
            Sembuf {
                sem_num: 0,
                sem_op: -1,
                sem_flg: IPC_NOWAIT as i16,
            },
            Sembuf {
                sem_num: 1,
                sem_op: -1,
                sem_flg: IPC_NOWAIT as i16,
            },
            Sembuf {
                sem_num: 2,
                sem_op: -1,
                sem_flg: IPC_NOWAIT as i16,
            },
        ];
        set_errno(0);
        // SAFETY: `ops` is three readable sembuf.
        assert_eq!(
            unsafe { kinakaze_abi_semop(id, ops.as_ptr(), ops.len()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EAGAIN);

        // Nothing may have moved. If either of the first two dropped to zero, the
        // array was applied piecewise and atomicity is broken.
        for slot in [0, 1] {
            // SAFETY: GETVAL reads no buffer.
            assert_eq!(
                unsafe { kinakaze_abi_semctl(id, slot, GETVAL, 0) },
                1,
                "semaphore {slot} was modified by a failed semop"
            );
        }

        // With the third available, the same array succeeds as a unit.
        assert_eq!(one_op(id, 2, 1, 0), 0);
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_semop(id, ops.as_ptr(), ops.len()) },
            0
        );
        for slot in [0, 1, 2] {
            // SAFETY: GETVAL reads no buffer.
            assert_eq!(unsafe { kinakaze_abi_semctl(id, slot, GETVAL, 0) }, 0);
        }

        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, IPC_RMID, 0) }, 0);
    }

    #[test]
    fn waiting_for_zero_succeeds_only_when_the_value_is_zero() {
        let id = kinakaze_abi_semget(IPC_PRIVATE, 1, IPC_CREAT | 0o600);
        assert!(id > 0);

        // A fresh semaphore is zero, so the wait passes immediately. Windows has no
        // primitive for this condition at all; it works because the whole set is
        // inspected under a lock.
        assert_eq!(one_op(id, 0, 0, 0), 0);

        // Non-zero, so the same wait must block — observed through IPC_NOWAIT
        // rather than by actually blocking the test.
        assert_eq!(one_op(id, 0, 3, 0), 0);
        set_errno(0);
        assert_eq!(one_op(id, 0, 0, IPC_NOWAIT as i16), -1);
        assert_eq!(kinakaze_tls::errno(), EAGAIN);

        // Draining it releases the wait.
        assert_eq!(one_op(id, 0, -3, 0), 0);
        assert_eq!(one_op(id, 0, 0, IPC_NOWAIT as i16), 0);

        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, IPC_RMID, 0) }, 0);
    }

    #[test]
    fn a_blocked_semop_is_released_by_another_thread() {
        // The blocking path itself, which IPC_NOWAIT tests cannot reach: the waiter
        // must sleep and then be woken by the event a completed operation signals.
        let id = kinakaze_abi_semget(IPC_PRIVATE, 1, IPC_CREAT | 0o600);
        assert!(id > 0);

        let poster = std::thread::spawn(move || {
            // Long enough that the main thread is genuinely inside the wait rather
            // than having found the value already available.
            std::thread::sleep(std::time::Duration::from_millis(150));
            assert_eq!(one_op(id, 0, 1, 0), 0);
        });

        let started = std::time::Instant::now();
        // Blocks until the thread above posts.
        assert_eq!(one_op(id, 0, -1, 0), 0);
        let waited = started.elapsed();
        poster.join().unwrap();

        // It really waited rather than spinning through to a stale success.
        assert!(
            waited.as_millis() >= 100,
            "the wait returned too early: {waited:?}"
        );
        // SAFETY: GETVAL reads no buffer.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, GETVAL, 0) }, 0);
        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, IPC_RMID, 0) }, 0);
    }
    /// The number of occupied undo slots in a set, read straight from the header.
    fn undo_slots_used(id: c_int) -> usize {
        let set = find_set(id).expect("the set should be live");
        let _guard = CrossProcessLock::acquire_handle(set.lock as *mut c_void).unwrap();
        // SAFETY: the control view is live while the Arc is held and the set's
        // mutex is held for the read.
        let header = unsafe { &*(set.control as *const SemHeader) };
        header.undo.iter().filter(|entry| entry.pid != 0).count()
    }

    #[test]
    fn sem_undo_records_a_debt_and_drops_it_when_it_cancels() {
        let id = kinakaze_abi_semget(IPC_PRIVATE, 1, IPC_CREAT | 0o600);
        assert!(id > 0);
        // SAFETY: SETVAL takes an int in the union.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, SETVAL, 1) }, 0);
        assert_eq!(undo_slots_used(id), 0);

        // Taking the lock with SEM_UNDO records the +1 that would restore it.
        assert_eq!(one_op(id, 0, -1, SEM_UNDO as i16), 0);
        assert_eq!(undo_slots_used(id), 1, "SEM_UNDO did not record a debt");

        // Releasing it cancels the debt exactly, so the slot is freed rather than
        // held for the life of the process.
        assert_eq!(one_op(id, 0, 1, SEM_UNDO as i16), 0);
        assert_eq!(undo_slots_used(id), 0, "the cancelled debt kept its slot");
        // SAFETY: GETVAL reads no buffer.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, GETVAL, 0) }, 1);

        // SETVAL invalidates any outstanding debt, because the promise to restore a
        // value cannot survive that value being overwritten.
        assert_eq!(one_op(id, 0, -1, SEM_UNDO as i16), 0);
        assert_eq!(undo_slots_used(id), 1);
        // SAFETY: SETVAL takes an int in the union.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, SETVAL, 5) }, 0);
        assert_eq!(undo_slots_used(id), 0, "SETVAL left a stale debt");

        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, IPC_RMID, 0) }, 0);
    }

    #[test]
    fn the_liveness_test_distinguishes_a_dead_process_from_a_recycled_id() {
        // This predicate is the whole of SEM_UNDO's crash safety, so it is tested
        // directly rather than only through a semop.
        assert!(
            process_is_alive(own_host_pid(), own_start_time()),
            "this process must look alive"
        );
        // The creation time is what guards against id reuse: the same id with a
        // different start time is a different process.
        assert!(
            !process_is_alive(own_host_pid(), own_start_time().wrapping_add(1)),
            "a mismatched creation time must not count as alive"
        );

        // A process that really has exited. Spawning and reaping one is the only way
        // to obtain an id that is certainly gone rather than merely unlikely.
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit", "0"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to spawn a child process");
        let pid = child.id();
        child.wait().expect("the child should be reapable");
        // The id may have been recycled between the wait and this call, which the
        // creation-time check turns into "not the process we meant" — the same
        // answer for this test's purpose. Passing 0 as the start time is the only
        // form that could be fooled, so a real recorded time is used instead.
        assert!(
            !process_is_alive(pid, own_start_time()),
            "an exited process must not look alive"
        );
    }

    #[test]
    fn getall_and_setall_move_the_whole_set() {
        let id = kinakaze_abi_semget(IPC_PRIVATE, 4, IPC_CREAT | 0o600);
        assert!(id > 0);

        let requested = [1u16, 2, 3, 4];
        // SAFETY: SETALL takes a pointer to `nsems` readable unsigned shorts.
        assert_eq!(
            unsafe { kinakaze_abi_semctl(id, 0, SETALL, requested.as_ptr() as usize) },
            0
        );

        let mut observed = [0u16; 4];
        // SAFETY: GETALL takes a pointer to `nsems` writable unsigned shorts.
        assert_eq!(
            unsafe { kinakaze_abi_semctl(id, 0, GETALL, observed.as_mut_ptr() as usize) },
            0
        );
        assert_eq!(observed, requested);

        // A value above SEMVMX is refused, and must leave the set as it was rather
        // than applying the entries ahead of the bad one.
        let overflowing = [9u16, 9, 40000, 9];
        set_errno(0);
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_semctl(id, 0, SETALL, overflowing.as_ptr() as usize) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), ERANGE);
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_semctl(id, 0, GETALL, observed.as_mut_ptr() as usize) },
            0
        );
        assert_eq!(observed, requested, "a refused SETALL modified the set");

        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, IPC_RMID, 0) }, 0);
    }

    #[test]
    fn semctl_stat_and_the_refused_commands_answer_honestly() {
        let id = kinakaze_abi_semget(IPC_PRIVATE, 3, IPC_CREAT | 0o640);
        assert!(id > 0);

        let mut stat = SemidDs::default();
        // SAFETY: IPC_STAT takes a pointer to a writable struct semid_ds.
        assert_eq!(
            unsafe { kinakaze_abi_semctl(id, 0, IPC_STAT, (&raw mut stat) as usize) },
            0
        );
        assert_eq!(stat.sem_nsems, 3);
        assert_eq!(stat.sem_perm.mode, 0o640);
        assert!(stat.sem_ctime > 0);
        // No operation has run yet, and Linux reports 0 rather than the create time.
        assert_eq!(stat.sem_otime, 0);

        assert_eq!(one_op(id, 0, 1, 0), 0);
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_semctl(id, 0, IPC_STAT, (&raw mut stat) as usize) },
            0
        );
        assert!(stat.sem_otime > 0, "sem_otime should follow an operation");

        // The two commands this module refuses. A fabricated queue length would be
        // read by monitoring tools as a real one.
        for command in [GETNCNT, GETZCNT] {
            set_errno(0);
            // SAFETY: neither command reads the union.
            assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, command, 0) }, -1);
            assert_eq!(kinakaze_tls::errno(), ENOSYS);
        }

        // A semaphore outside the set is EFBIG, which is the error Linux uses and
        // is distinct from a stale identifier's EINVAL.
        set_errno(0);
        // SAFETY: GETVAL reads no buffer.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 9, GETVAL, 0) }, -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        set_errno(0);
        assert_eq!(one_op(id, 9, 1, 0), -1);
        assert_eq!(kinakaze_tls::errno(), EFBIG);

        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, IPC_RMID, 0) }, 0);
        // And afterwards the id reports removal, not nonsense.
        set_errno(0);
        assert_eq!(one_op(id, 0, 1, 0), -1);
        assert_eq!(kinakaze_tls::errno(), EIDRM);
    }

    #[test]
    fn semget_rejects_a_set_larger_than_the_header_can_hold() {
        set_errno(0);
        assert_eq!(
            kinakaze_abi_semget(IPC_PRIVATE, SEMMSL as c_int + 1, IPC_CREAT | 0o600),
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        // The documented limit itself is accepted, so the boundary is where the
        // constant says it is.
        let id = kinakaze_abi_semget(IPC_PRIVATE, SEMMSL as c_int, IPC_CREAT | 0o600);
        assert!(id > 0);
        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(unsafe { kinakaze_abi_semctl(id, 0, IPC_RMID, 0) }, 0);
    }

    #[test]
    fn ftok_is_deterministic_and_reports_a_missing_path() {
        // `/` is the guest root, which always resolves.
        // SAFETY: a null-terminated literal.
        let first = unsafe { kinakaze_abi_ftok(c"/".as_ptr(), 1) };
        assert_ne!(first, -1, "ftok on / failed: {}", kinakaze_tls::errno());
        // SAFETY: as above.
        let again = unsafe { kinakaze_abi_ftok(c"/".as_ptr(), 1) };
        assert_eq!(first, again, "ftok must be deterministic for one path");

        // The project id occupies the top byte, so changing it must change the key.
        // SAFETY: as above.
        let other_project = unsafe { kinakaze_abi_ftok(c"/".as_ptr(), 2) };
        assert_ne!(first, other_project);
        assert_eq!(
            (other_project as u32) >> 24,
            2,
            "the project id belongs in the top byte"
        );

        // A path with no file behind it cannot yield a key, and saying so beats
        // returning one that names nothing.
        set_errno(0);
        // SAFETY: a null-terminated literal.
        assert_eq!(
            unsafe { kinakaze_abi_ftok(c"/no/such/path".as_ptr(), 1) },
            -1
        );
        assert_ne!(kinakaze_tls::errno(), 0);
    }

    #[test]
    fn the_control_granule_leaves_the_guest_a_page_aligned_segment() {
        // The data view starts one granule in, so the guest's pointer is aligned at
        // least as strictly as a page — which is what a Linux shmat guarantees and
        // what code placing atomics in a segment depends on.
        assert_eq!(control_bytes() % allocation_granularity(), 0);
        assert!(control_bytes() >= size_of::<ShmHeader>());
        assert!(control_bytes() >= size_of::<SemHeader>());

        let id = kinakaze_abi_shmget(IPC_PRIVATE, 64, IPC_CREAT | 0o600);
        assert!(id > 0);
        // SAFETY: `id` is live and a null address lets the kernel choose.
        let view = unsafe { kinakaze_abi_shmat(id, std::ptr::null(), 0) };
        assert_ne!(view, SHM_FAILED);
        assert_eq!(view as usize % 4096, 0, "the segment is not page-aligned");
        // SAFETY: `view` came from the shmat above.
        assert_eq!(unsafe { kinakaze_abi_shmdt(view) }, 0);
        // SAFETY: IPC_RMID reads no buffer.
        assert_eq!(
            unsafe { kinakaze_abi_shmctl(id, IPC_RMID, std::ptr::null_mut()) },
            0
        );
    }
}
