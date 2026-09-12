//! Scheduling, resource limits, namespaces and privileged system control.
//!
//! Linux namespaces and supported mounts are implemented by the native VFS and
//! shared process registry. Host scheduling and resource queries use Windows
//! APIs. Unsupported facilities return ENOSYS/EOPNOTSUPP, failed namespace
//! capability checks return EPERM, and malformed requests return EINVAL.
//!
//! Successful calls must retain the requested state and affect later operations;
//! a success-shaped no-op cannot substitute for a missing mount or namespace.

use core::ffi::{CStr, c_char, c_int, c_void};
use core::sync::atomic::{AtomicI32, AtomicPtr, AtomicUsize, Ordering};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::set_errno;
mod keys;
mod mount_api;
pub mod mqueue;
use kinakaze_vfs::{EAGAIN, EFAULT, EINVAL, ENOSYS, ENOTDIR, EPERM, ETIMEDOUT};
pub use mount_api::*;

/// `ESRCH`, which the VFS error list does not carry because no filesystem path
/// produces it.
const ESRCH: i32 = 3;

/// No Linux disk quota backend is installed. Report the same unsupported
/// facility result as a kernel without CONFIG_QUOTA; never invent quota data.
/// https://man7.org/linux/man-pages/man2/quotactl.2.html
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_quotactl(
    _operation: c_int,
    _special: *const c_char,
    _id: c_int,
    _data: *mut c_void,
) -> c_int {
    set_errno(ENOSYS);
    -1
}

// ELF modules cannot be loaded into the Windows kernel. Export the Linux ABI
// so consumers can detect this unsupported operation instead of failing to load.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_init_module(
    _image: *const c_void,
    _length: usize,
    _arguments: *const c_char,
) -> c_int {
    set_errno(ENOSYS);
    -1
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_finit_module(
    _fd: c_int,
    _arguments: *const c_char,
    _flags: c_int,
) -> c_int {
    set_errno(ENOSYS);
    -1
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_delete_module(
    _name: *const c_char,
    _flags: c_int,
) -> c_int {
    set_errno(ENOSYS);
    -1
}

// Win32 entry points this module calls directly.
//
// Declared inline rather than pulled from `windows-sys`, whose enabled feature
// set covers `Win32_System_Threading` only. Every signature matches the Windows
// headers: `BOOL` is `i32`, `HANDLE` is a pointer, and `KAFFINITY` is
// pointer-width, which is why the affinity masks below are `usize`.
#[link(name = "kernel32")]
unsafe extern "system" {
    /// `SwitchToThread`: yields the rest of this thread's quantum.
    fn SwitchToThread() -> i32;
    /// `GetCurrentProcess`: the pseudo-handle for this process.
    fn GetCurrentProcess() -> *mut c_void;
    /// `GetCurrentThread`: the pseudo-handle for this thread.
    fn GetCurrentThread() -> *mut c_void;
    /// `GetCurrentThreadId`: this thread's Windows id.
    fn GetCurrentThreadId() -> u32;
    /// `ExitThread`: terminates only the calling task, matching raw `SYS_exit`.
    fn ExitThread(status: u32) -> !;
    /// `GetLastError`: the failure code of the last failing call.
    fn GetLastError() -> u32;
    /// `GetProcessAffinityMask`: the process and system affinity masks.
    fn GetProcessAffinityMask(
        process: *mut c_void,
        process_mask: *mut usize,
        system_mask: *mut usize,
    ) -> i32;
    /// `SetThreadAffinityMask`: pins a thread, returning its previous mask.
    ///
    /// Returns 0 on failure, which is why the result is checked against 0 rather
    /// than treated as a `BOOL`.
    fn SetThreadAffinityMask(thread: *mut c_void, mask: usize) -> usize;
    /// `SetProcessAffinityMask`: pins every thread of the process.
    fn SetProcessAffinityMask(process: *mut c_void, mask: usize) -> i32;
    /// `SetThreadDescription`: names a thread for debuggers and ETW.
    fn SetThreadDescription(thread: *mut c_void, description: *const u16) -> i32;
    /// `GetThreadDescription`: reads a thread's name into a fresh allocation.
    ///
    /// The buffer it writes must be released with `LocalFree`.
    fn GetThreadDescription(thread: *mut c_void, description: *mut *mut u16) -> i32;
    /// `LocalFree`: releases what `GetThreadDescription` allocated.
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    /// `FlushProcessWriteBuffers`: flushes every processor's store buffer.
    ///
    /// This is the operation `membarrier` performs, reached through the same
    /// mechanism: an IPI to each processor running one of this process's threads.
    fn FlushProcessWriteBuffers();
    /// `GetSystemTimeAsFileTime`: wall-clock time, 100 ns ticks since 1601.
    fn GetSystemTimeAsFileTime(time: *mut FileTime);
    /// `QueryPerformanceCounter`: a monotonic tick count.
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
    /// `GetProcessTimes`: cumulative kernel and user time for this process.
    fn GetProcessTimes(
        process: *mut c_void,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    /// `GetThreadTimes`: the same four figures for one thread.
    fn GetThreadTimes(
        thread: *mut c_void,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    /// `RtlGenRandom`, exported under this name: the system CSPRNG.
    ///
    /// Chosen over `BCryptGenRandom` because it needs no algorithm handle and no
    /// bcrypt import, and over `CryptGenRandom` because that one is deprecated.
    /// It is the same generator `getrandom` should be backed by: kernel-seeded
    /// and never a userspace PRNG, which matters because callers use this for
    /// keys and for stack-guard values.
    #[link_name = "SystemFunction036"]
    fn RtlGenRandom(buffer: *mut c_void, length: u32) -> u8;
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
}

// ---------------------------------------------------------------------------
// Scheduling.
// ---------------------------------------------------------------------------

/// `sched_yield`, which offers the rest of this thread's quantum to others.
///
/// `SwitchToThread` is an exact equivalent, and this is one of the few entries in
/// this module that needs no qualification.
///
/// The return values are not equivalent, though, and conflating them would be a
/// bug. Linux `sched_yield` returns 0 unconditionally. `SwitchToThread` returns
/// zero to mean "no other thread was ready to run", which is a perfectly normal
/// outcome and not a failure. Propagating it would have a caller in a
/// yield-and-retry spin loop see a spurious error on an idle machine, so it is
/// deliberately discarded.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sched_yield() -> c_int {
    // SAFETY: `SwitchToThread` takes no arguments and has no preconditions.
    let _yielded_to_someone = unsafe { SwitchToThread() };
    0
}

/// The Windows process and system affinity masks, as `(process, system)`.
///
/// Returns `None` when the query fails, which leaves the caller to report an
/// error rather than act on a mask of zero.
fn affinity_masks() -> Option<(usize, usize)> {
    let mut process = 0usize;
    let mut system = 0usize;
    // SAFETY: the pseudo-handle is always valid and both out-parameters are
    // writable locals.
    let ok =
        unsafe { GetProcessAffinityMask(GetCurrentProcess(), &raw mut process, &raw mut system) };
    (ok != 0).then_some((process, system))
}

// The affinity mask last installed by `sched_setaffinity` on this thread.
//
// Windows offers no `GetThreadAffinityMask`. `SetThreadAffinityMask` returns the
// previous mask, so the only way to read the current one through the Win32 API is
// to set it and set it back — a round trip that would perturb the scheduler on a
// pure query and could lose the original mask if the second call failed.
//
// Remembering what was set instead is exact for the get-after-set sequence, which
// is the one callers actually perform, and a thread that has never set its
// affinity genuinely does inherit the process mask, so the fallback in
// `current_affinity` is not a guess either. The value can only go stale if
// something outside this layer repins the thread, which nothing here does.
thread_local! {
    static THREAD_AFFINITY: core::cell::Cell<Option<usize>> = const { core::cell::Cell::new(None) };
    /// Linux `clear_child_tid` for this task. It is process-shared memory but
    /// the selected address is per-thread kernel state.
    static CHILD_CLEAR_TID: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

/// The affinity mask in force for the calling thread.
fn current_affinity() -> Option<usize> {
    let (process, _system) = affinity_masks()?;
    Some(THREAD_AFFINITY.get().unwrap_or(process))
}

/// Writes an affinity mask into a Linux `cpu_set_t` buffer.
///
/// # The 64-CPU ceiling
///
/// A Linux `cpu_set_t` is a 1024-bit array — 128 bytes, which is why the syscall
/// takes its size. A Windows `KAFFINITY` is a single pointer-width word, so at
/// most 64 CPUs can be described. Bits 64 and above are therefore reported clear.
///
/// That ceiling is not merely theoretical. On a machine with more than 64 logical
/// processors Windows splits them into *processor groups*, and
/// `GetProcessAffinityMask` describes only the group the calling thread belongs
/// to; it reports failure outright for a process spanning multiple groups. So on
/// such a host the mask this produces is the truth about one group rather than the
/// whole machine. Describing the rest would mean the group-aware
/// `GetLogicalProcessorInformationEx` and `SetThreadGroupAffinity`, which is a
/// larger change than this module needs and is noted here rather than pretended.
///
/// # Safety
///
/// `set` must name at least `size` writable bytes.
unsafe fn write_cpu_set(set: *mut u8, size: usize, mask: usize) -> Result<(), i32> {
    validate_cpu_set_size(size)?;
    // SAFETY: the caller guarantees `size` writable bytes.
    unsafe {
        // The whole buffer is cleared first: a caller reading CPU 200 out of a
        // 128-byte set must see a clear bit, not whatever was on its stack.
        core::ptr::write_bytes(set, 0, size);
        core::ptr::copy_nonoverlapping((&raw const mask).cast::<u8>(), set, size_of::<usize>());
    }
    Ok(())
}

/// Checks a `cpu_set_t` length the way the kernel does.
///
/// Two conditions, both taken from the kernel's own validation rather than
/// invented here. A buffer smaller than one word cannot express even CPU 0. And
/// the length must be a whole number of words: the kernel walks a cpumask as an
/// array of `unsigned long`, so a length like 12 describes a trailing partial word
/// that has no meaning, and Linux rejects it rather than rounding. Matching that
/// matters because a caller that passes a bad length gets `EINVAL` from the real
/// kernel and should get the same here, instead of silently having its buffer
/// treated as one word shorter.
fn validate_cpu_set_size(size: usize) -> Result<(), i32> {
    if size < size_of::<usize>() || !size.is_multiple_of(size_of::<usize>()) {
        return Err(EINVAL);
    }
    Ok(())
}

/// Whether `pid` names something this process can describe.
///
/// Recognizes the local targets; other visible process leaders are resolved
/// through the shared registry and a creation-token-checked native handle.
fn names_self(pid: c_int) -> bool {
    pid == 0
        || pid == crate::process::kinakaze_abi_getpid()
        // SAFETY: `GetCurrentThreadId` has no preconditions.
        || pid == unsafe { GetCurrentThreadId() } as c_int
}

/// Retain the exact native process behind a visible guest PID. Scheduling a
/// recycled Windows PID must never affect an unrelated process.
fn affinity_process(pid: c_int, write: bool) -> Result<std::os::windows::io::OwnedHandle, i32> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    };
    if pid <= 0 {
        return Err(ESRCH);
    }
    let info = kinakaze_vfs::job::process_info(pid as u32).ok_or(ESRCH)?;
    if write {
        let target = kinakaze_runtime::job::namespaces::resolve(pid as u32).ok_or(ESRCH)?;
        let namespace = kinakaze_vfs::user_namespace::id(target)?;
        if !kinakaze_vfs::user_namespace::capable(namespace, 23) {
            return Err(EPERM);
        }
    }
    let raw = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | if write { PROCESS_SET_INFORMATION } else { 0 },
            0,
            info.entry.pid,
        )
    };
    if raw.is_null() {
        return Err(ESRCH);
    }
    let process = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
    let mut created = windows_sys::Win32::Foundation::FILETIME::default();
    let mut ignored = windows_sys::Win32::Foundation::FILETIME::default();
    let ok = unsafe {
        windows_sys::Win32::System::Threading::GetProcessTimes(
            process.as_raw_handle().cast(),
            &mut created,
            &mut ignored,
            &mut ignored,
            &mut ignored,
        )
    };
    let token = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    if ok == 0 || token != info.entry.token {
        return Err(ESRCH);
    }
    Ok(process)
}

/// `sched_getaffinity`.
///
/// Genuinely backed by `GetProcessAffinityMask`. See [`write_cpu_set`] for the
/// 64-CPU ceiling and the processor-group caveat, and [`validate_cpu_set_size`]
/// for the two length rules the kernel enforces.
///
/// Returns 0 on success, which is the *glibc wrapper's* convention. The raw
/// syscall returns the number of bytes written instead;
/// [`kinakaze_abi_syscall`] applies that form when the number is dispatched
/// directly.
///
/// # Safety
///
/// `set` must name at least `size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sched_getaffinity(
    pid: c_int,
    size: usize,
    set: *mut c_void,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    match unsafe { get_affinity(pid, size, set) } {
        Ok(_written) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// The shared body of `sched_getaffinity`, reporting the bytes written.
///
/// # Safety
///
/// `set` must name at least `size` writable bytes.
unsafe fn get_affinity(pid: c_int, size: usize, set: *mut c_void) -> Result<usize, i32> {
    let process = if names_self(pid) {
        None
    } else {
        Some(affinity_process(pid, false)?)
    };
    if set.is_null() {
        return Err(EFAULT);
    }
    let mask = if let Some(process) = process {
        use std::os::windows::io::AsRawHandle;
        let (mut mask, mut system) = (0, 0);
        if unsafe { GetProcessAffinityMask(process.as_raw_handle().cast(), &mut mask, &mut system) }
            == 0
        {
            return Err(kinakaze_vfs::EIO);
        }
        mask
    } else {
        current_affinity().ok_or(kinakaze_vfs::EIO)?
    };
    // SAFETY: forwarded from this function's contract.
    unsafe { write_cpu_set(set.cast::<u8>(), size, mask) }?;
    // The kernel reports how much of the cpumask it filled, which is one word
    // here rather than the caller's whole buffer.
    Ok(size_of::<usize>())
}

/// `sched_setaffinity`.
///
/// Backed by `SetThreadAffinityMask` when the target is the calling thread and by
/// `SetProcessAffinityMask` when it is the process. The distinction is kept
/// because Linux affinity is thread-granular: pinning the whole process for a
/// caller that asked to pin one thread would constrain threads it said nothing
/// about.
///
/// The requested mask is intersected with the CPUs this backend can schedule.
/// An empty intersection is EINVAL; an all-ones mask resets affinity, as used
/// by runc before starting its namespace helper.
///
/// # Safety
///
/// `set` must name at least `size` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sched_setaffinity(
    pid: c_int,
    size: usize,
    set: *const c_void,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    match unsafe { set_affinity(pid, size, set) } {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// The shared body of `sched_setaffinity`.
///
/// # Safety
///
/// `set` must name at least `size` readable bytes.
unsafe fn set_affinity(pid: c_int, size: usize, set: *const c_void) -> Result<(), i32> {
    let process = if names_self(pid) {
        None
    } else {
        Some(affinity_process(pid, true)?)
    };
    if set.is_null() {
        return Err(EFAULT);
    }
    validate_cpu_set_size(size)?;
    let bytes = set.cast::<u8>();
    let mut requested = 0usize;
    // SAFETY: the size check above proved one word is readable.
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes,
            (&raw mut requested).cast::<u8>(),
            size_of::<usize>(),
        );
    }
    let (available_process, system) = affinity_masks().ok_or(kinakaze_vfs::EIO)?;
    let thread_target =
        process.is_none() && (pid == 0 || pid == unsafe { GetCurrentThreadId() } as c_int);
    requested &= if thread_target {
        available_process
    } else {
        system
    };
    // An empty set is EINVAL on Linux: a thread must be able to run somewhere.
    if requested == 0 {
        return Err(EINVAL);
    }

    if let Some(process) = process {
        use std::os::windows::io::AsRawHandle;
        if unsafe { SetProcessAffinityMask(process.as_raw_handle().cast(), requested) } == 0 {
            return Err(kinakaze_vfs::errno_from_win32(unsafe { GetLastError() }));
        }
        return Ok(());
    }

    // A tid target pins just that thread; a pid target pins the process.
    // SAFETY: `GetCurrentThreadId` has no preconditions.
    if thread_target {
        // SAFETY: the pseudo-handle is valid and `requested` is a subset of the
        // system mask, which is the documented requirement.
        let previous = unsafe { SetThreadAffinityMask(GetCurrentThread(), requested) };
        if previous == 0 {
            // SAFETY: no preconditions.
            return Err(kinakaze_vfs::errno_from_win32(unsafe { GetLastError() }));
        }
        THREAD_AFFINITY.set(Some(requested));
        return Ok(());
    }
    // SAFETY: the pseudo-handle is valid and the mask is a checked subset.
    let ok = unsafe { SetProcessAffinityMask(GetCurrentProcess(), requested) };
    if ok == 0 {
        // SAFETY: no preconditions.
        return Err(kinakaze_vfs::errno_from_win32(unsafe { GetLastError() }));
    }
    // The process mask now bounds this thread too, so the remembered thread mask
    // would only be misleading. Note that this clears the record for the calling
    // thread only: another thread that had pinned itself still holds its own cached
    // value and would report it until it sets affinity again. Reaching into other
    // threads' storage is not possible from here, and the alternative — a shared
    // table keyed by tid — would need its own locking for a value that is only ever
    // read by the thread it describes.
    THREAD_AFFINITY.set(None);
    Ok(())
}

// ---------------------------------------------------------------------------
// Resource limits.
// ---------------------------------------------------------------------------

/// The Linux `struct rlimit64`: two 64-bit limits.
///
/// On x86_64 this is bit-for-bit the same as `struct rlimit`, because `rlim_t` is
/// already 64-bit there. The `64` suffix survives as a separate exported symbol
/// from the days of 32-bit `rlim_t`, and guest binaries reference both spellings,
/// so both must exist.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct RLimit64 {
    pub rlim_cur: u64,
    pub rlim_max: u64,
}

impl RLimit64 {
    fn read_user(address: usize) -> Result<Self, i32> {
        let mut bytes = [0; 16];
        mount_api::read_user(address, &mut bytes)?;
        Ok(Self {
            rlim_cur: u64::from_ne_bytes(bytes[..8].try_into().unwrap()),
            rlim_max: u64::from_ne_bytes(bytes[8..].try_into().unwrap()),
        })
    }

    fn write_user(self, address: usize) -> Result<(), i32> {
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&self.rlim_cur.to_ne_bytes());
        bytes[8..].copy_from_slice(&self.rlim_max.to_ne_bytes());
        mount_api::write_user(address, &bytes)
    }
}

/// `RLIM64_INFINITY`, all ones on Linux x86_64.
pub const RLIM64_INFINITY: u64 = u64::MAX;

/// `getrlimit`/`setrlimit` resource numbers, which are ABI.
pub const RLIMIT_CPU: c_int = 0;
pub const RLIMIT_FSIZE: c_int = 1;
pub const RLIMIT_DATA: c_int = 2;
pub const RLIMIT_STACK: c_int = 3;
pub const RLIMIT_CORE: c_int = 4;
pub const RLIMIT_RSS: c_int = 5;
pub const RLIMIT_NPROC: c_int = 6;
pub const RLIMIT_NOFILE: c_int = 7;
pub const RLIMIT_MEMLOCK: c_int = 8;
pub const RLIMIT_AS: c_int = 9;
/// One past the highest resource Linux defines, which bounds a valid argument.
const RLIMIT_NLIMITS: c_int = 16;

/// The stack the guest actually runs on, in bytes.
///
/// This is deliberately *not* `GetCurrentThreadStackLimits`. The loader does not
/// run guest code on the Windows thread stack: it reserves a separate block and
/// enters the ELF entry point with `rsp` inside it, so the limit that governs the
/// guest is that block's size. See `STACK_REGION` in
/// `crates/kinakaze-link/src/launch.rs`, which this mirrors.
///
/// The constant is duplicated rather than imported because `libc` does not depend
/// on `kinakaze-link` — the loader links the libc, not the other way round. It
/// coincides with the usual Linux `RLIMIT_STACK` default of 8 MiB, which is what
/// the loader chose it to match.
const GUEST_STACK_SIZE: u64 = 8 * 1024 * 1024;

/// The limit in force for one resource.
///
/// Several of these are real figures rather than placeholders.
///
/// - `RLIMIT_NOFILE` is the process's retained soft/hard ceiling. Descriptor
///   pages are allocated on use, up to [`kinakaze_vfs::MAX_FDS`]. Raising a
///   limit does not allocate entries or change the fixed libc fd_set ABI.
/// - `RLIMIT_STACK` is the loader's guest stack block. See [`GUEST_STACK_SIZE`]
///   for why this is not the Windows thread stack.
/// - `RLIMIT_AS` and `RLIMIT_DATA` are `RLIM64_INFINITY`: there is no enforced
///   per-process address-space or data-segment quota. The host's usable virtual
///   address range is an architectural constraint, not either Linux rlimit.
/// - `RLIMIT_CORE` is genuinely 0. Nothing here writes a Linux core file; Windows
///   produces minidumps through WER, an entirely separate mechanism that no
///   `RLIMIT_CORE` value influences. A caller checking before it enables core
///   dumps gets the correct answer, and BusyBox's `ulimit -c` prints the truth.
///
/// Everything else is `RLIM64_INFINITY`, because no per-process ceiling on it is
/// enforced here that could be reported. "Unlimited" is the honest encoding of an
/// unenforced limit — and the same value a normal Linux process carries for most
/// of these — where a made-up number would be acted on as though something
/// checked it.
fn limit_for(resource: c_int, pid: u32) -> Result<RLimit64, i32> {
    let both = |value: u64| RLimit64 {
        rlim_cur: value,
        rlim_max: value,
    };
    Ok(match resource {
        RLIMIT_FSIZE | RLIMIT_NPROC => {
            let (rlim_cur, rlim_max) = kinakaze_vfs::limits::limits(pid, resource as u32, None)?;
            RLimit64 { rlim_cur, rlim_max }
        }
        12 => {
            let (soft, hard) = kinakaze_vfs::mqueue::limits(pid, None)?;
            RLimit64 {
                rlim_cur: soft,
                rlim_max: hard,
            }
        }
        RLIMIT_NOFILE => {
            let (soft, hard) = kinakaze_vfs::job::nofile_limits(pid)?;
            RLimit64 {
                rlim_cur: soft,
                rlim_max: hard,
            }
        }
        RLIMIT_STACK => RLimit64 {
            rlim_cur: GUEST_STACK_SIZE,
            // The hard limit is unlimited on Linux by default, and nothing here
            // would stop a larger block being reserved for a future guest.
            rlim_max: RLIM64_INFINITY,
        },
        // Linux INIT_RLIMITS also starts AS and DATA without process quotas.
        // Ordinary allocation failures do not imply a configured resource limit.
        RLIMIT_AS | RLIMIT_DATA => both(RLIM64_INFINITY),
        RLIMIT_CORE => both(0),
        // RLIMIT_CPU, RLIMIT_FSIZE, RLIMIT_RSS, RLIMIT_NPROC, RLIMIT_MEMLOCK and
        // the four Linux adds above them. None is enforced here.
        _ => both(RLIM64_INFINITY),
    })
}

/// `getrlimit64`.
///
/// # Safety
///
/// `limit` must point at a writable `struct rlimit64`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getrlimit64(
    resource: c_int,
    limit: *mut RLimit64,
) -> c_int {
    if !(0..RLIMIT_NLIMITS).contains(&resource) {
        crate::set_errno(EINVAL);
        return -1;
    }
    if limit.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    let value = match limit_for(resource, 0) {
        Ok(value) => value,
        Err(error) => {
            crate::set_errno(error);
            return -1;
        }
    };
    // Chromium deliberately probes read-only mappings here. Preserve Linux's
    // EFAULT result instead of faulting inside the native provider.
    if let Err(error) = value.write_user(limit as usize) {
        crate::set_errno(error);
        return -1;
    }
    0
}

/// `setrlimit64`.
///
/// An exact no-op succeeds. A state-changing request only succeeds when this
/// layer can both retain and enforce the requested state; unsupported resources
/// fail with `EOPNOTSUPP` instead of reporting a change that did not happen.
///
/// `RLIMIT_NOFILE` is retained in the shared process registry and enforced by
/// every descriptor allocator. Its hard limit cannot exceed
/// `kinakaze_vfs::MAX_FDS`.
///
/// # Safety
///
/// `limit` must point at a readable `struct rlimit64`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setrlimit64(
    resource: c_int,
    limit: *const RLimit64,
) -> c_int {
    if !(0..RLIMIT_NLIMITS).contains(&resource) {
        crate::set_errno(EINVAL);
        return -1;
    }
    if limit.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    let requested = match RLimit64::read_user(limit as usize) {
        Ok(value) => value,
        Err(error) => {
            crate::set_errno(error);
            return -1;
        }
    };
    // Linux checks this before anything else, and so does this.
    if requested.rlim_cur > requested.rlim_max {
        crate::set_errno(EINVAL);
        return -1;
    }
    let current = match limit_for(resource, 0) {
        Ok(current) => current,
        Err(error) => {
            crate::set_errno(error);
            return -1;
        }
    };
    if requested == current {
        return 0;
    }
    if resource == 12 {
        return match kinakaze_vfs::mqueue::limits(0, Some((requested.rlim_cur, requested.rlim_max)))
        {
            Ok(_) => 0,
            Err(e) => {
                crate::set_errno(e);
                -1
            }
        };
    }
    if matches!(resource, RLIMIT_FSIZE | RLIMIT_NPROC) {
        return match kinakaze_vfs::limits::limits(
            0,
            resource as u32,
            Some((requested.rlim_cur, requested.rlim_max)),
        ) {
            Ok(_) => 0,
            Err(error) => {
                crate::set_errno(error);
                -1
            }
        };
    }
    // RLIMIT_NOFILE is enforced by every descriptor allocator in the VFS.
    if resource == RLIMIT_NOFILE {
        return match kinakaze_vfs::job::set_nofile_limits(0, requested.rlim_cur, requested.rlim_max)
        {
            Ok(()) => 0,
            Err(error) => {
                crate::set_errno(error);
                -1
            }
        };
    }
    // A limit with no enforcement boundary cannot truthfully be changed. Exact
    // no-op writes above still succeed; state-changing requests are explicit.
    crate::set_errno(kinakaze_vfs::EOPNOTSUPP);
    -1
}

pub(crate) unsafe fn prlimit64(
    pid: u32,
    resource: c_int,
    requested: *const RLimit64,
    previous: *mut RLimit64,
) -> Result<(), i32> {
    if !(0..RLIMIT_NLIMITS).contains(&resource) {
        return Err(EINVAL);
    }
    // Copy input first: Linux permits the new and old limit buffers to alias.
    let requested = if requested.is_null() {
        None
    } else {
        Some(RLimit64::read_user(requested as usize)?)
    };
    let old = limit_for(resource, pid)?;
    if !previous.is_null() {
        old.write_user(previous as usize)?;
    }
    let Some(requested) = requested else {
        return Ok(());
    };
    if requested.rlim_cur > requested.rlim_max {
        return Err(EINVAL);
    }
    if requested == old {
        return Ok(());
    }
    if resource == 12 {
        kinakaze_vfs::mqueue::limits(pid, Some((requested.rlim_cur, requested.rlim_max)))?;
        return Ok(());
    }
    if matches!(resource, RLIMIT_FSIZE | RLIMIT_NPROC) {
        kinakaze_vfs::limits::limits(
            pid,
            resource as u32,
            Some((requested.rlim_cur, requested.rlim_max)),
        )?;
        return Ok(());
    }
    if resource != RLIMIT_NOFILE {
        return Err(kinakaze_vfs::EOPNOTSUPP);
    }
    kinakaze_vfs::job::set_nofile_limits(pid, requested.rlim_cur, requested.rlim_max)
}

// ---------------------------------------------------------------------------
// prctl.
// ---------------------------------------------------------------------------

/// `prctl` operation numbers. ABI: the guest passes these integers.
pub const PR_SET_PDEATHSIG: c_int = 1;
pub const PR_GET_PDEATHSIG: c_int = 2;
pub const PR_GET_DUMPABLE: c_int = 3;
pub const PR_SET_DUMPABLE: c_int = 4;
pub const PR_GET_KEEPCAPS: c_int = 7;
pub const PR_SET_KEEPCAPS: c_int = 8;
pub const PR_SET_NAME: c_int = 15;
pub const PR_GET_NAME: c_int = 16;
pub const PR_CAPBSET_READ: c_int = 23;
pub const PR_CAPBSET_DROP: c_int = 24;
pub const PR_SET_TIMERSLACK: c_int = 29;
pub const PR_GET_TIMERSLACK: c_int = 30;
pub const PR_SET_CHILD_SUBREAPER: c_int = 36;
pub const PR_GET_CHILD_SUBREAPER: c_int = 37;
pub const PR_SET_NO_NEW_PRIVS: c_int = 38;
pub const PR_GET_NO_NEW_PRIVS: c_int = 39;
pub const PR_CAP_AMBIENT: c_int = 47;

/// The length of a Linux thread name, including its terminator.
///
/// `PR_SET_NAME` truncates silently at this width and `PR_GET_NAME` writes
/// exactly this many bytes, so a caller's 16-byte buffer is the contract.
const TASK_COMM_LEN: usize = 16;

/// The `PR_SET_DUMPABLE` state.
///
/// Process-wide because that is what it is on Linux, and tracked rather than
/// enforced. The value is honestly reportable — a `PR_GET_DUMPABLE` returns what
/// was set — but nothing consults it, and the reason it does not is that no Linux
/// core dump is ever produced here in the first place. So `SUID_DUMP_DISABLE` is
/// already the effective state whatever this holds, and a caller clearing it to
/// keep secrets out of a dump gets the outcome it wanted. A caller *setting* it to
/// 1 does not get dumps, which is the direction where tracking-without-enforcing
/// diverges; that is the same divergence `RLIMIT_CORE` of 0 already reports.
static DUMPABLE: AtomicI32 = AtomicI32::new(1);

/// Installs a Windows thread description from a Linux `comm` string.
fn set_thread_name(name: &str) -> Result<(), i32> {
    // Windows takes UTF-16 and the Linux name is bytes; the conversion is lossy
    // only for a `comm` that was not valid UTF-8, which cannot round-trip anyway.
    let mut wide: Vec<u16> = name.encode_utf16().collect();
    wide.push(0);
    // SAFETY: the pseudo-handle is valid and `wide` is a null-terminated buffer
    // that outlives the call.
    let status = unsafe { SetThreadDescription(GetCurrentThread(), wide.as_ptr()) };
    // The call returns an HRESULT, not a BOOL: negative is failure.
    if status < 0 {
        return Err(kinakaze_vfs::EIO);
    }
    Ok(())
}

/// Reads back the Windows thread description.
///
/// Returns an empty string when the thread was never named, which is what a
/// freshly created thread's `comm` looks like before anything sets it.
fn thread_name() -> String {
    let mut raw: *mut u16 = core::ptr::null_mut();
    // SAFETY: the pseudo-handle is valid and `raw` is a writable local.
    let status = unsafe { GetThreadDescription(GetCurrentThread(), &raw mut raw) };
    if status < 0 || raw.is_null() {
        return String::new();
    }
    // SAFETY: on success Windows wrote a null-terminated UTF-16 string here.
    let length = unsafe {
        let mut count = 0usize;
        while *raw.add(count) != 0 {
            count += 1;
        }
        count
    };
    // SAFETY: `length` is the count of units before the terminator.
    let name = String::from_utf16_lossy(unsafe { core::slice::from_raw_parts(raw, length) });
    // Windows allocated this buffer with LocalAlloc and the caller owns it.
    // SAFETY: `raw` came from `GetThreadDescription` and is freed exactly once.
    unsafe { LocalFree(raw.cast::<c_void>()) };
    name
}

/// `prctl`.
///
/// Variadic in C. Rust cannot spell an `extern "sysv64"` function with `...`, and
/// it does not need to here: `prctl` takes at most four arguments after the
/// option, so all five arrive in System V integer registers and a fixed
/// five-parameter signature is ABI-identical to the variadic one. A caller passing
/// fewer arguments leaves the later registers holding whatever was in them, which
/// is why each option below reads only the arguments it is defined to use. Where a
/// variadic *interpretation* is genuinely needed — a `va_list` frame — this crate
/// builds one in assembly; see [`crate::variadic`].
///
/// # Implemented for real
///
/// `PR_SET_NAME` and `PR_GET_NAME` map onto `SetThreadDescription` and
/// `GetThreadDescription`. This is not a stub that remembers a string: the name
/// shows up on the thread in WinDbg, Visual Studio and ETW traces, which is the
/// visible effect a caller setting a thread name is after.
///
/// `PR_GET_DUMPABLE` and `PR_SET_DUMPABLE` are tracked as process state; see
/// [`DUMPABLE`] for what that does and does not guarantee.
///
/// Everything else returns `EINVAL`, which is what Linux returns for an option it
/// does not recognise.
///
/// # Safety
///
/// When `option` is `PR_SET_NAME`, `argument2` must be a readable pointer to a
/// null-terminated string of at most 16 bytes. When it is `PR_GET_NAME`,
/// `argument2` must name 16 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_prctl(
    option: c_int,
    argument2: u64,
    _argument3: u64,
    _argument4: u64,
    _argument5: u64,
) -> c_int {
    match option {
        PR_SET_NAME => {
            let pointer = argument2 as *const c_char;
            if pointer.is_null() {
                crate::set_errno(EFAULT);
                return -1;
            }
            // SAFETY: the caller guarantees a null-terminated string.
            let raw = unsafe { CStr::from_ptr(pointer) };
            // The kernel truncates at 16 bytes including the NUL rather than
            // failing, so 15 bytes of name survive.
            let bytes = raw.to_bytes();
            let end = bytes.len().min(TASK_COMM_LEN - 1);
            let name = String::from_utf8_lossy(&bytes[..end]).into_owned();
            match set_thread_name(&name) {
                Ok(()) => 0,
                Err(error) => {
                    crate::set_errno(error);
                    -1
                }
            }
        }
        PR_GET_NAME => {
            let pointer = argument2 as *mut u8;
            if pointer.is_null() {
                crate::set_errno(EFAULT);
                return -1;
            }
            let name = thread_name();
            let bytes = name.as_bytes();
            let count = bytes.len().min(TASK_COMM_LEN - 1);
            // SAFETY: the caller guarantees 16 writable bytes, and `count` is at
            // most 15, so the terminator and the zero fill stay in bounds.
            unsafe {
                core::ptr::write_bytes(pointer, 0, TASK_COMM_LEN);
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, count);
            }
            0
        }
        PR_GET_DUMPABLE => DUMPABLE.load(Ordering::Relaxed),
        PR_SET_DUMPABLE => {
            let requested = argument2 as i32;
            // SUID_DUMP_DISABLE, SUID_DUMP_USER and SUID_DUMP_ROOT. Linux rejects
            // anything else with EINVAL.
            if !(0..=2).contains(&requested) {
                crate::set_errno(EINVAL);
                return -1;
            }
            DUMPABLE.store(requested, Ordering::Relaxed);
            0
        }
        PR_SET_NO_NEW_PRIVS => {
            if argument2 != 1 || _argument3 != 0 || _argument4 != 0 || _argument5 != 0 {
                crate::set_errno(EINVAL);
                return -1;
            }
            0
        }
        PR_GET_NO_NEW_PRIVS => 1,
        PR_CAPBSET_READ => {
            let cap = argument2 as i32;
            if (0..=40).contains(&cap) {
                1
            } else {
                crate::set_errno(EINVAL);
                -1
            }
        }
        PR_CAPBSET_DROP => 0,
        PR_SET_KEEPCAPS => 0,
        PR_GET_KEEPCAPS => 0,
        PR_CAP_AMBIENT => 0,
        PR_SET_TIMERSLACK => 0,
        PR_GET_TIMERSLACK => 50_000,
        PR_SET_CHILD_SUBREAPER => {
            kinakaze_vfs::job::ensure_registered();
            let flag = kinakaze_runtime::job::FLAG_SUBREAPER;
            kinakaze_runtime::job::update_flags(
                kinakaze_runtime::job::current_pid(),
                if argument2 != 0 { flag } else { 0 },
                if argument2 == 0 { flag } else { 0 },
            );
            0
        }
        PR_GET_CHILD_SUBREAPER => {
            let ptr = argument2 as *mut i32;
            if ptr.is_null() {
                crate::set_errno(EFAULT);
                return -1;
            }
            kinakaze_vfs::job::ensure_registered();
            let flag = kinakaze_runtime::job::FLAG_SUBREAPER;
            let enabled = kinakaze_runtime::job::lookup(kinakaze_runtime::job::current_pid())
                .is_some_and(|entry| entry.flags & flag != 0);
            unsafe { *ptr = i32::from(enabled) };
            0
        }
        PR_SET_PDEATHSIG => match kinakaze_vfs::job::set_parent_death_signal(argument2 as i32) {
            Ok(()) => 0,
            Err(error) => {
                crate::set_errno(error);
                -1
            }
        },
        PR_GET_PDEATHSIG => {
            let pointer = argument2 as *mut i32;
            if pointer.is_null() {
                crate::set_errno(EFAULT);
                return -1;
            }
            match kinakaze_vfs::job::parent_death_signal() {
                Ok(signal) => {
                    unsafe { pointer.write(signal) };
                    0
                }
                Err(error) => {
                    crate::set_errno(error);
                    -1
                }
            }
        }
        _ => {
            crate::set_errno(EINVAL);
            -1
        }
    }
}

// ---------------------------------------------------------------------------
// Filesystem administration. Proc, bind and overlay have VFS mount backends;
// unsupported filesystem types and legacy operations remain explicit errors.
// ---------------------------------------------------------------------------

/// `mount`.
///
/// Dispatches only implemented mount backends. Existing synthetic paths do not
/// imply that a new filesystem instance can be mounted at another location.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn mount(
    source: *const c_char,
    target: *const c_char,
    filesystem: *const c_char,
    flags: u64,
    data: *const c_void,
) -> c_int {
    unsafe { kinakaze_abi_mount(source, target, filesystem, flags, data) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mount(
    source: *const c_char,
    target: *const c_char,
    filesystem: *const c_char,
    flags: u64,
    data: *const c_void,
) -> c_int {
    // Linux path_mount selects the operation by flags before consulting the
    // filesystem name: remount, bind, propagation, move, then a new filesystem.
    // Proc remount and mount propagation are dispatched to their VFS backends.
    const MS_REMOUNT: u64 = 32;
    const MS_MOVE: u64 = 8192;
    const MS_PROPAGATION: u64 = (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20);
    const MS_NOUSER: u64 = 1 << 31;
    let flags = if flags & 0xffff_0000 == 0xc0ed_0000 {
        flags & !0xffff_0000
    } else {
        flags
    };
    if target.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    if flags & MS_NOUSER != 0 {
        crate::set_errno(EINVAL);
        return -1;
    }
    let fs_type = if filesystem.is_null() || flags & kinakaze_vfs::mount::MS_BIND != 0 {
        ""
    } else {
        match unsafe { CStr::from_ptr(filesystem) }.to_str() {
            Ok(s) => s,
            Err(_) => {
                crate::set_errno(EINVAL);
                return -1;
            }
        }
    };
    let target_str = match unsafe { CStr::from_ptr(target) }.to_str() {
        Ok(s) => s,
        Err(_) => {
            crate::set_errno(EINVAL);
            return -1;
        }
    };
    if flags & (MS_REMOUNT | kinakaze_vfs::mount::MS_BIND)
        == MS_REMOUNT | kinakaze_vfs::mount::MS_BIND
    {
        return match kinakaze_vfs::mount::remount_bind(target_str, flags) {
            Ok(()) => 0,
            Err(error) => {
                crate::set_errno(error);
                -1
            }
        };
    }
    let tmpfs_remount = flags & MS_REMOUNT != 0
        && kinakaze_vfs::mount::resolve_mount_target(target_str)
            .is_ok_and(|p| kinakaze_vfs::tmpfs::owns(&p));
    if (matches!(fs_type, "tmpfs" | "devpts" | "mqueue" | "sysfs" | "cgroup2")
        && flags & (MS_MOVE | MS_PROPAGATION | kinakaze_vfs::mount::MS_BIND) == 0)
        || tmpfs_remount
    {
        let options = if data.is_null() {
            ""
        } else {
            match unsafe { CStr::from_ptr(data.cast()) }.to_str() {
                Ok(s) => s,
                Err(_) => {
                    crate::set_errno(EINVAL);
                    return -1;
                }
            }
        };
        let result = if flags & MS_REMOUNT != 0 {
            kinakaze_vfs::mount::tmpfs_remount(target_str, flags, options)
        } else if fs_type == "cgroup2" {
            kinakaze_vfs::mount::cgroup_mount(target_str, flags, options)
        } else if fs_type == "sysfs" {
            kinakaze_vfs::mount::sysfs_mount(target_str, flags, options)
        } else if fs_type == "mqueue" {
            kinakaze_vfs::mount::mqueue_mount(target_str, flags, options)
        } else if fs_type == "devpts" {
            kinakaze_vfs::mount::devpts_mount(target_str, flags, options)
        } else {
            kinakaze_vfs::mount::tmpfs_mount(target_str, flags, options)
        };
        return match result {
            Ok(()) => 0,
            Err(e) => {
                crate::set_errno(e);
                -1
            }
        };
    }
    let is_proc_remount = flags & MS_REMOUNT != 0
        && kinakaze_vfs::mount::resolve_mount_target(target_str).is_ok_and(|p| {
            kinakaze_vfs::procfs::instance::filesystem(&p)
                .ok()
                .flatten()
                .is_some()
        });
    if (fs_type == "proc" && flags & (MS_MOVE | MS_PROPAGATION | kinakaze_vfs::mount::MS_BIND) == 0)
        || is_proc_remount
    {
        let options = if data.is_null() {
            ""
        } else {
            match unsafe { CStr::from_ptr(data.cast()) }.to_str() {
                Ok(s) => s,
                Err(_) => {
                    crate::set_errno(EINVAL);
                    return -1;
                }
            }
        };
        let result = if flags & MS_REMOUNT != 0 {
            kinakaze_vfs::mount::proc_remount(target_str, flags, options)
        } else {
            kinakaze_vfs::mount::proc_mount(target_str, flags, options)
        };
        return match result {
            Ok(()) => 0,
            Err(e) => {
                crate::set_errno(e);
                -1
            }
        };
    }
    let is_bind = flags & kinakaze_vfs::mount::MS_BIND != 0;
    let is_propagation = !is_bind && (flags & kinakaze_vfs::mount::MS_PROPAGATION != 0);
    if flags & MS_REMOUNT != 0
        || (!is_bind && !is_propagation && flags & (MS_PROPAGATION | MS_MOVE) != 0)
        || (is_propagation && flags & MS_MOVE != 0)
    {
        crate::set_errno(ENOSYS);
        return -1;
    }
    let target = match unsafe { CStr::from_ptr(target) }.to_str() {
        Ok(path) => kinakaze_vfs::fs::absolute_linux(path),
        Err(_) => {
            crate::set_errno(EINVAL);
            return -1;
        }
    };
    if is_propagation {
        match kinakaze_vfs::mount::set_propagation(target_str, flags) {
            Ok(()) => return 0,
            Err(errno) => {
                crate::set_errno(errno);
                return -1;
            }
        }
    }
    if source.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    let source = match unsafe { CStr::from_ptr(source) }.to_str() {
        Ok(path) => kinakaze_vfs::fs::absolute_linux(path),
        Err(_) => {
            crate::set_errno(EINVAL);
            return -1;
        }
    };
    // The bind backend does not interpret either fstype or filesystem options.
    // In particular an ignored "overlay" string must not select its parser,
    // and arbitrary bytes in ignored options are not required to be UTF-8.
    let filesystem = if is_bind || filesystem.is_null() {
        ""
    } else {
        match unsafe { CStr::from_ptr(filesystem) }.to_str() {
            Ok(value) => value,
            Err(_) => {
                crate::set_errno(EINVAL);
                return -1;
            }
        }
    };
    if filesystem == "overlay" {
        let data_str = if data.is_null() {
            ""
        } else {
            match unsafe { CStr::from_ptr(data as *const c_char) }.to_str() {
                Ok(value) => value,
                Err(_) => {
                    crate::set_errno(EINVAL);
                    return -1;
                }
            }
        };
        let options = match kinakaze_vfs::mount::overlay::parse_options(data_str, flags) {
            Ok(options) => options,
            Err(error) => {
                crate::set_errno(error);
                return -1;
            }
        };
        let operation = || {
            kinakaze_vfs::mount::overlay(
                &target,
                &options.lowerdirs,
                options.upperdir.as_deref(),
                options.workdir.as_deref(),
                options.flags,
            )
        };
        // Overlay preparation defers signal delivery until its metadata guards
        // unwind. A pending signal must be consumed before a caller retries.
        // Ordinary mounts can restart before mount-table publication. Volatile
        // preparation writes persistent workdir markers: dispatch its signal,
        // but do not implicitly replay a possibly partially written marker.
        let result = if options.flags & kinakaze_vfs::mount::overlay::features::VOLATILE != 0 {
            let result = operation();
            if result == Err(kinakaze_vfs::EINTR) {
                kinakaze_vfs::signal::deliver_pending();
            }
            result
        } else {
            crate::fs::restart_metadata(operation)
        };
        return match result {
            Ok(()) => 0,
            Err(error) => {
                crate::set_errno(error);
                -1
            }
        };
    }
    if !is_bind {
        crate::set_errno(ENOSYS);
        return -1;
    }
    let source_info = match kinakaze_vfs::fs::stat(&source) {
        Ok(info) => info,
        Err(error) => {
            crate::set_errno(error);
            return -1;
        }
    };
    let target_info = match kinakaze_vfs::fs::stat(&target) {
        Ok(info) => info,
        Err(error) => {
            crate::set_errno(error);
            return -1;
        }
    };
    if (source_info.st_mode & kinakaze_vfs::fs::S_IFMT == kinakaze_vfs::fs::S_IFDIR)
        != (target_info.st_mode & kinakaze_vfs::fs::S_IFMT == kinakaze_vfs::fs::S_IFDIR)
    {
        crate::set_errno(ENOTDIR);
        return -1;
    }
    // For a new bind Linux uses only MS_REC; other mount attributes do not
    // change the underlying mount and require a separately implemented remount.
    let bind_flags = kinakaze_vfs::mount::MS_BIND | (flags & kinakaze_vfs::mount::MS_REC);
    match kinakaze_vfs::mount::bind(&source, &target, bind_flags) {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `umount2`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn umount2(target: *const c_char, flags: c_int) -> c_int {
    unsafe { kinakaze_abi_umount2(target, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_umount2(target: *const c_char, flags: c_int) -> c_int {
    if target.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    let target = match unsafe { CStr::from_ptr(target) }.to_str() {
        Ok(path) => path.to_owned(),
        Err(_) => {
            crate::set_errno(EINVAL);
            return -1;
        }
    };
    match kinakaze_vfs::mount::unmount(&target, flags) {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `pivot_root`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pivot_root(new_root: *const c_char, put_old: *const c_char) -> c_int {
    unsafe { kinakaze_abi_pivot_root(new_root, put_old) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pivot_root(
    new_root: *const c_char,
    put_old: *const c_char,
) -> c_int {
    if new_root.is_null() || put_old.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    let (Ok(new_root), Ok(put_old)) = (
        unsafe { CStr::from_ptr(new_root) }.to_str(),
        unsafe { CStr::from_ptr(put_old) }.to_str(),
    ) else {
        crate::set_errno(EINVAL);
        return -1;
    };
    match crate::fs::restart_metadata(|| kinakaze_vfs::mount::pivot_root(new_root, put_old)) {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `swapon`.
///
/// Refused with `ENOSYS`. Windows paging files are configured through the registry
/// and the system properties UI, take effect on reboot, and are not the
/// per-device, runtime-activated thing this call describes. A program cannot
/// enable one, so there is no operation to perform and no privilege that would
/// grant it.
///
/// # Safety
///
/// `path` is not dereferenced; the signature matches Linux.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_swapon(_path: *const c_char, _flags: c_int) -> c_int {
    crate::set_errno(ENOSYS);
    -1
}

/// `swapoff`, refused with `ENOSYS` for the reasons under [`kinakaze_abi_swapon`].
///
/// # Safety
///
/// `path` is not dereferenced; the signature matches Linux.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_swapoff(_path: *const c_char) -> c_int {
    crate::set_errno(ENOSYS);
    -1
}

// ---------------------------------------------------------------------------
// Namespaces.
// ---------------------------------------------------------------------------

/// `unshare`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn unshare(flags: c_int) -> c_int {
    kinakaze_abi_unshare(flags)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_unshare(flags: c_int) -> c_int {
    const VALID: u32 = 0x0000_0080
        | 0x0000_0100
        | 0x0000_0200
        | 0x0000_0400
        | 0x0000_0800
        | 0x0001_0000
        | 0x0002_0000
        | 0x0004_0000
        | 0x0200_0000
        | 0x0400_0000
        | 0x0800_0000
        | 0x1000_0000
        | 0x2000_0000
        | 0x4000_0000;
    if flags == 0 {
        return 0;
    }
    if flags as u32 & !VALID != 0 {
        crate::set_errno(EINVAL);
        return -1;
    }
    const CLONE_NEWNS: u32 = 0x0002_0000;
    const CLONE_NEWTIME: u32 = 0x80;
    const CLONE_SYSVSEM: u32 = 0x0004_0000;
    const OBJECT_FLAGS: u32 = 0x04000000 | 0x08000000 | 0x02000000 | 0x20000000 | 0x40000000;
    const PRIVATE_FLAGS: u32 = 0x100 | 0x200 | 0x400 | 0x800 | 0x10000 | CLONE_SYSVSEM | 0x10000000;
    let flags = flags as u32;
    if flags & !(OBJECT_FLAGS | CLONE_NEWNS | CLONE_NEWTIME | PRIVATE_FLAGS) != 0 {
        if let Some(directory) = std::env::var_os("KINAKAZE_NAMESPACE_TRACE_DIR") {
            use std::io::Write;
            let path = std::path::PathBuf::from(directory)
                .join(format!("namespace-{}.log", std::process::id()));
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(
                    file,
                    "unshare flags={flags:#010x} unsupported={:#010x}",
                    flags & !(OBJECT_FLAGS | CLONE_NEWNS | CLONE_NEWTIME | PRIVATE_FLAGS)
                );
            }
        }
        crate::set_errno(ENOSYS);
        return -1;
    }
    let action = || -> Result<(), i32> {
        // Network namespaces are task scoped on Linux. The VFS keeps a
        // calling-thread override so Go's runtime.LockOSThread + setns/unshare
        // sequence remains isolated from the daemon's other threads.
        if kinakaze_runtime::process_thread_count() != 1
            && flags & !(0x200 | CLONE_NEWNS | 0x4000_0000) != 0
        {
            return Err(EINVAL);
        }
        let user = if flags & 0x10000000 != 0 {
            Some(kinakaze_vfs::user_namespace::prepare_unshare()?)
        } else {
            None
        };
        let objects = if flags & OBJECT_FLAGS != 0 {
            Some(kinakaze_vfs::namespaces::prepare_with_user(
                flags,
                user.as_ref(),
            )?)
        } else {
            None
        };
        let time = if flags & CLONE_NEWTIME != 0 {
            Some(kinakaze_vfs::time_namespace::prepare_with_user(
                user.as_ref(),
            )?)
        } else {
            None
        };
        let mount = if flags & CLONE_NEWNS != 0 {
            Some(kinakaze_vfs::mount::prepare_with_user(user.as_ref())?)
        } else {
            None
        };
        if flags & (CLONE_SYSVSEM | 0x08000000) != 0 {
            crate::sysvipc::unshare_undo()?;
        }
        if flags & (0x200 | CLONE_NEWNS | 0x10000000) != 0 {
            kinakaze_vfs::fs_context::unshare();
        }
        if let Some(user) = user {
            user.install()?;
        }
        if let Some(install) = mount {
            install()?;
        }
        if let Some(install) = time {
            install()?;
        }
        if let Some(objects) = objects {
            objects.install()?;
        }
        Ok(())
    };
    match action() {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `setns`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn setns(fd: c_int, kind: c_int) -> c_int {
    kinakaze_abi_setns(fd, kind)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setns(fd: c_int, kind: c_int) -> c_int {
    if let Err(error) = kinakaze_vfs::get(fd) {
        crate::set_errno(error);
        return -1;
    }
    if kind == 0x10000000
        || (kind == 0
            && kinakaze_vfs::get(fd).is_ok_and(|e| e.kind == kinakaze_vfs::FdKind::UserNamespace))
    {
        return match kinakaze_vfs::user_namespace::enter(fd) {
            Ok(()) => 0,
            Err(e) => {
                crate::set_errno(e);
                -1
            }
        };
    }
    if kinakaze_vfs::get(fd).is_ok_and(|e| e.kind == kinakaze_vfs::FdKind::Namespace) {
        return match kinakaze_vfs::namespaces::enter(fd, kind as u32, |namespace_kind| {
            if namespace_kind == kinakaze_vfs::namespaces::IPC {
                crate::sysvipc::unshare_undo()?;
            }
            Ok(())
        }) {
            Ok(()) => 0,
            Err(error) => {
                crate::set_errno(error);
                -1
            }
        };
    }
    if kind == 0x80
        || (kind == 0
            && kinakaze_vfs::get(fd).is_ok_and(|e| e.kind == kinakaze_vfs::FdKind::TimeNamespace))
    {
        return match kinakaze_vfs::time_namespace::enter(fd) {
            Ok(()) => 0,
            Err(error) => {
                crate::set_errno(error);
                -1
            }
        };
    }
    if kind == 0 || kind == 0x0002_0000 {
        return match kinakaze_vfs::mount::enter_namespace(fd) {
            Ok(()) => 0,
            Err(error) => {
                crate::set_errno(error);
                -1
            }
        };
    }
    crate::set_errno(EINVAL);
    -1
}

/// `seccomp`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn seccomp(operation: u32, flags: u32, args: *mut c_void) -> c_int {
    unsafe { kinakaze_abi_seccomp(operation, flags, args) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_seccomp(
    _operation: u32,
    _flags: u32,
    _args: *mut c_void,
) -> c_int {
    // No filter evaluator is installed in syscall dispatch yet. Successful
    // activation here would falsely claim the process is confined.
    crate::set_errno(kinakaze_vfs::ENOSYS);
    -1
}

/// `memfd_create`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn memfd_create(name: *const c_char, flags: u32) -> c_int {
    unsafe { kinakaze_abi_memfd_create(name, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_memfd_create(name: *const c_char, flags: u32) -> c_int {
    // Chromium probes kernel support with all flag bits set. Returning an FD
    // for that request makes its capability check abort, with stale errno.
    // Only CLOEXEC and ALLOW_SEALING belong to this implementation's surface.
    if flags & !3 != 0 {
        crate::set_errno(EINVAL);
        return -1;
    }
    if name.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let name = unsafe { CStr::from_ptr(name) };
    if name.to_bytes().len() > 249 {
        crate::set_errno(EINVAL);
        return -1;
    }
    let tag = if let Ok(s) = name.to_str() {
        s.replace(['/', '\\', '\0'], "_")
    } else {
        "memfd".to_string()
    };
    let random_suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = format!("/tmp/memfd_{}_{}", tag, random_suffix);
    let fd_flags = if flags & 1 != 0 {
        kinakaze_vfs::fs::O_RDWR
            | kinakaze_vfs::fs::O_CREAT
            | kinakaze_vfs::fs::O_EXCL
            | kinakaze_vfs::fs::O_CLOEXEC
    } else {
        kinakaze_vfs::fs::O_RDWR | kinakaze_vfs::fs::O_CREAT | kinakaze_vfs::fs::O_EXCL
    };
    let fd = match kinakaze_vfs::fs::open(&tmp_path, fd_flags, 0o700) {
        Ok(fd) => fd,
        Err(err) => {
            crate::set_errno(err);
            return -1;
        }
    };
    fd
}

// ---------------------------------------------------------------------------
// Execution domain.
// ---------------------------------------------------------------------------

/// `PER_LINUX`, the persona this process runs under and the only one available.
const PER_LINUX: u64 = 0x0000_0000;

/// The query form: Linux treats `0xffffffff` as "report, do not change".
const PERSONALITY_QUERY: u64 = 0xffff_ffff;

/// `personality`.
///
/// The query form is answered truthfully: this is a `PER_LINUX` process, so
/// reading the current persona returns 0. Note the unusual convention — the return
/// value *is* the previous persona, so 0 is a success and not an error, and the
/// documented way to detect failure is `-1` with errno.
///
/// Setting any other persona fails with `EINVAL`, matching Linux's answer for a
/// domain the kernel does not implement.
///
/// # ADDR_NO_RANDOMIZE
///
/// One flag deserves calling out, because a caller that is refused it may behave
/// differently than it expects rather than simply failing. `ADDR_NO_RANDOMIZE`
/// (0x0040000) is what `setarch -R` and most reproducible-crash tooling set before
/// re-executing, and it cannot be honoured here for a reason that is structural
/// rather than incidental: on Linux the flag takes effect at the *next* `execve`,
/// when the kernel lays out the new image. This layer's guest is already mapped by
/// the time any guest code runs, so there is no later layout decision left to
/// influence. Refusing is therefore truthful. The consequence is that a debugger
/// or a fuzzer expecting stable addresses across runs will not get them, and will
/// see the `EINVAL` rather than silently drawing the wrong conclusion from
/// addresses that moved.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_personality(persona: u64) -> c_int {
    if persona == PERSONALITY_QUERY || persona == PER_LINUX {
        // The previous persona, which is the current one either way.
        return PER_LINUX as c_int;
    }
    crate::set_errno(EINVAL);
    -1
}

// ---------------------------------------------------------------------------
// Privileged system control.
// ---------------------------------------------------------------------------

/// `chroot`.
///
/// Refused with `EPERM`.
///
/// # Why this is not implemented, having looked
///
/// The VFS does have the machinery a real `chroot` would need.
/// `kinakaze_vfs::resolve_linux_path_from` takes an explicit virtual root and
/// already clamps `..` at it, so a per-process root would be expressible.
///
/// What is missing is a way to *install* one. The root comes from
/// `kinakaze_vfs::system_root`, which returns the hosting executable's directory
/// and takes no override; `resolve_linux_path` calls it on every translation. So
/// changing the root means changing that layer, and every path-taking entry point
/// in this libc reaches resolution through it — `open`, `stat`, `readlink`,
/// `getcwd`, `execve` and the `*at` family among them.
///
/// A partial `chroot` is worse than none. If it applied to `open` but not to
/// `stat`, a caller inside the jail could `stat` its way around the filesystem
/// while believing it was confined, and code that treats `chroot` as a security
/// boundary would be relying on a boundary with holes in it. Failing cleanly keeps
/// that decision with the caller.
///
/// `EPERM` rather than `ENOSYS` because this is a policy refusal about an
/// operation the layer could support, not a statement that no such facility could
/// exist here — and because it is what Linux returns to a caller without
/// `CAP_SYS_CHROOT`, so the code path is well travelled.
///
/// Implementing it would mean an overridable root in `kinakaze-vfs` (a process-wide
/// `OnceLock`-plus-`RwLock` prefix consulted by `resolve_linux_path`), and an audit
/// that no entry point resolves a path by any other route.
///
/// # Safety
///
/// `path` must be a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_chroot(path: *const c_char) -> c_int {
    if path.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let c_str = unsafe { core::ffi::CStr::from_ptr(path) };
    let path_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => {
            crate::set_errno(kinakaze_vfs::EINVAL);
            return -1;
        }
    };
    match kinakaze_vfs::fs::chroot(path_str) {
        Ok(()) => 0,
        Err(err) => {
            crate::set_errno(err);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn chroot(path: *const c_char) -> c_int {
    unsafe { kinakaze_abi_chroot(path) }
}

/// `klogctl`, the kernel ring buffer interface.
///
/// Refused with `EPERM`. Windows keeps no kernel message buffer readable this way.
/// The System event log is not the same object: it is a structured, per-provider
/// store reached through `EvtQuery`, holding records with different content and
/// different retention, and reshaping it into a `dmesg`-style byte stream would be
/// inventing a kernel log rather than reporting one.
///
/// `dmesg` is what calls this, and it will print the failure to the user. That is
/// the correct outcome: the user learns there is no kernel log here, which is true,
/// instead of reading a synthesized one and drawing conclusions from it. `EPERM`
/// specifically is what Linux returns when `dmesg_restrict` is set, so `dmesg`
/// already has a message for it.
///
/// # Safety
///
/// `buffer` is not dereferenced; the signature matches Linux.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_klogctl(
    _kind: c_int,
    _buffer: *mut c_char,
    _length: c_int,
) -> c_int {
    crate::set_errno(EPERM);
    -1
}

/// `LINUX_REBOOT_MAGIC1`, the first of the two magic numbers the kernel demands.
const LINUX_REBOOT_MAGIC1: u32 = 0xfee1_dead;

/// `LINUX_REBOOT_MAGIC2` and the three alternates the kernel also accepts.
///
/// The kernel takes any of these four; they are birthdays of Torvalds' children,
/// and all four remain valid for compatibility. A caller using one of the later
/// spellings is as correct as one using the first.
const LINUX_REBOOT_MAGIC2: u32 = 672_274_793;
const LINUX_REBOOT_MAGIC2A: u32 = 85_072_278;
const LINUX_REBOOT_MAGIC2B: u32 = 369_367_448;
const LINUX_REBOOT_MAGIC2C: u32 = 537_993_216;

/// `reboot` commands.
pub const LINUX_REBOOT_CMD_RESTART: c_int = 0x0123_4567;
pub const LINUX_REBOOT_CMD_HALT: c_int = 0xCDEF_0123u32 as c_int;
pub const LINUX_REBOOT_CMD_CAD_ON: c_int = 0x89AB_CDEFu32 as c_int;
pub const LINUX_REBOOT_CMD_CAD_OFF: c_int = 0x0000_0000;
pub const LINUX_REBOOT_CMD_POWER_OFF: c_int = 0x4321_FEDCu32 as c_int;
pub const LINUX_REBOOT_CMD_RESTART2: c_int = 0xA1B2_C3D4u32 as c_int;
pub const LINUX_REBOOT_CMD_SW_SUSPEND: c_int = 0xD000_FCE2u32 as c_int;
pub const LINUX_REBOOT_CMD_KEXEC: c_int = 0x4558_4543;

/// Whether a magic pair is the one the kernel demands.
///
/// The kernel checks this before it consults capabilities, so a caller with the
/// wrong constants must learn *that* rather than being told it lacks permission.
/// Used by the raw-syscall path in [`kinakaze_abi_syscall`]; the glibc wrapper
/// supplies the magics itself and never exposes them to its caller.
fn reboot_magic_is_valid(magic1: u32, magic2: u32) -> bool {
    magic1 == LINUX_REBOOT_MAGIC1
        && matches!(
            magic2,
            LINUX_REBOOT_MAGIC2
                | LINUX_REBOOT_MAGIC2A
                | LINUX_REBOOT_MAGIC2B
                | LINUX_REBOOT_MAGIC2C
        )
}

/// Whether `command` is a `reboot` operation the kernel defines.
fn reboot_command_is_valid(command: c_int) -> bool {
    matches!(
        command,
        LINUX_REBOOT_CMD_RESTART
            | LINUX_REBOOT_CMD_HALT
            | LINUX_REBOOT_CMD_CAD_ON
            | LINUX_REBOOT_CMD_CAD_OFF
            | LINUX_REBOOT_CMD_POWER_OFF
            | LINUX_REBOOT_CMD_RESTART2
            | LINUX_REBOOT_CMD_SW_SUSPEND
            | LINUX_REBOOT_CMD_KEXEC
    )
}

/// `reboot`.
///
/// This is the glibc wrapper's signature — one argument, the command — not the
/// four-argument raw syscall. glibc supplies the two magic numbers itself and never
/// exposes them, so an exported `reboot` that demanded them in registers would
/// reject every real caller: BusyBox calls `reboot(RB_AUTOBOOT)` with one argument,
/// and the magic check would read whatever was in `rsi` and fail with the wrong
/// errno. The magic validation lives in [`reboot_magic_is_valid`] and is applied on
/// the raw path, where the magics are actually passed.
///
/// **This deliberately never reboots anything.** Windows genuinely can: it exports
/// `ExitWindowsEx` and `InitiateSystemShutdownExW`, and with `SeShutdownPrivilege`
/// enabled either would restart the machine. Neither is called, and neither is
/// even declared in this module, so no future edit can reach one by accident.
///
/// The refusal is the point. A compatibility layer must not restart the
/// developer's workstation because a guest process called `reboot()` — and guests
/// do: BusyBox's `init`, `halt`, `poweroff` and `reboot` applets all end here, and
/// a mistaken invocation while testing an applet would take the host down with
/// every unsaved editor buffer on it. There is no plausible reading of "run this
/// Linux binary" that includes it.
///
/// The command is still decoded and validated, because a caller deserves the error
/// its request actually earned: an unrecognised command is `EINVAL` before the
/// refusal, as the kernel orders its checks. A well-formed request then gets
/// `EPERM`, which is what an unprivileged Linux caller receives and is therefore
/// both the safe answer and a defensible one — every caller already has a path for
/// it.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_reboot(command: c_int) -> c_int {
    if !reboot_command_is_valid(command) {
        crate::set_errno(EINVAL);
        return -1;
    }
    // A valid, understood request. Refused anyway; see this function's
    // documentation. Nothing above this line has any side effect.
    crate::set_errno(EPERM);
    -1
}

// ---------------------------------------------------------------------------
// The raw syscall gate.
// ---------------------------------------------------------------------------

/// Linux x86_64 syscall numbers this gate recognises.
///
/// These are architecture-specific and non-negotiable: the guest was compiled
/// against the x86_64 table and passes these integers.
pub const SYS_READ: i64 = 0;
pub const SYS_WRITE: i64 = 1;
pub const SYS_OPEN: i64 = 2;
pub const SYS_CLOSE: i64 = 3;
pub const SYS_STAT: i64 = 4;
pub const SYS_FSTAT: i64 = 5;
pub const SYS_LSTAT: i64 = 6;
pub const SYS_POLL: i64 = 7;
pub const SYS_LSEEK: i64 = 8;
pub const SYS_MMAP: i64 = 9;
pub const SYS_MPROTECT: i64 = 10;
pub const SYS_MUNMAP: i64 = 11;
pub const SYS_BRK: i64 = 12;
pub const SYS_RT_SIGACTION: i64 = 13;
pub const SYS_RT_SIGPROCMASK: i64 = 14;
pub const SYS_RT_SIGRETURN: i64 = 15;
pub const SYS_IOCTL: i64 = 16;
pub const SYS_PREAD64: i64 = 17;
pub const SYS_PWRITE64: i64 = 18;
pub const SYS_READV: i64 = 19;
pub const SYS_WRITEV: i64 = 20;
pub const SYS_ACCESS: i64 = 21;
pub const SYS_PIPE: i64 = 22;
pub const SYS_SELECT: i64 = 23;
pub const SYS_SCHED_YIELD: i64 = 24;
pub const SYS_MREMAP: i64 = 25;
pub const SYS_MSYNC: i64 = 26;
pub const SYS_MINCORE: i64 = 27;
pub const SYS_MADVISE: i64 = 28;
pub const SYS_SHMGET: i64 = 29;
pub const SYS_SHMAT: i64 = 30;
pub const SYS_SHMCTL: i64 = 31;
pub const SYS_DUP: i64 = 32;
pub const SYS_DUP2: i64 = 33;
pub const SYS_PAUSE: i64 = 34;
pub const SYS_NANOSLEEP: i64 = 35;
pub const SYS_GETITIMER: i64 = 36;
pub const SYS_ALARM: i64 = 37;
pub const SYS_SETITIMER: i64 = 38;
pub const SYS_GETPID: i64 = 39;
pub const SYS_SENDFILE: i64 = 40;
pub const SYS_SOCKET: i64 = 41;
pub const SYS_CONNECT: i64 = 42;
pub const SYS_ACCEPT: i64 = 43;
pub const SYS_SENDTO: i64 = 44;
pub const SYS_RECVFROM: i64 = 45;
pub const SYS_SENDMSG: i64 = 46;
pub const SYS_RECVMSG: i64 = 47;
pub const SYS_SHUTDOWN: i64 = 48;
pub const SYS_BIND: i64 = 49;
pub const SYS_LISTEN: i64 = 50;
pub const SYS_GETSOCKNAME: i64 = 51;
pub const SYS_GETPEERNAME: i64 = 52;
pub const SYS_SOCKETPAIR: i64 = 53;
pub const SYS_SETSOCKOPT: i64 = 54;
pub const SYS_GETSOCKOPT: i64 = 55;
pub const SYS_CLONE: i64 = 56;
pub const SYS_FORK: i64 = 57;
pub const SYS_VFORK: i64 = 58;
pub const SYS_EXECVE: i64 = 59;
pub const SYS_EXIT: i64 = 60;
pub const SYS_WAIT4: i64 = 61;
pub const SYS_KILL: i64 = 62;
pub const SYS_UNAME: i64 = 63;
pub const SYS_SEMGET: i64 = 64;
pub const SYS_SEMOP: i64 = 65;
pub const SYS_SEMCTL: i64 = 66;
pub const SYS_SHMDT: i64 = 67;
pub const SYS_FCNTL: i64 = 72;
pub const SYS_FLOCK: i64 = 73;
pub const SYS_FSYNC: i64 = 74;
pub const SYS_FDATASYNC: i64 = 75;
pub const SYS_TRUNCATE: i64 = 76;
pub const SYS_FTRUNCATE: i64 = 77;
pub const SYS_GETDENTS: i64 = 78;
pub const SYS_GETCWD: i64 = 79;
pub const SYS_CHDIR: i64 = 80;
pub const SYS_FCHDIR: i64 = 81;
pub const SYS_RENAME: i64 = 82;
pub const SYS_MKDIR: i64 = 83;
pub const SYS_RMDIR: i64 = 84;
pub const SYS_CREAT: i64 = 85;
pub const SYS_LINK: i64 = 86;
pub const SYS_UNLINK: i64 = 87;
pub const SYS_SYMLINK: i64 = 88;
pub const SYS_READLINK: i64 = 89;
pub const SYS_CHMOD: i64 = 90;
pub const SYS_FCHMOD: i64 = 91;
pub const SYS_CHOWN: i64 = 92;
pub const SYS_FCHOWN: i64 = 93;
pub const SYS_LCHOWN: i64 = 94;
pub const SYS_UMASK: i64 = 95;
pub const SYS_GETTIMEOFDAY: i64 = 96;
pub const SYS_GETRLIMIT: i64 = 97;
pub const SYS_GETRUSAGE: i64 = 98;
pub const SYS_SYSINFO: i64 = 99;
pub const SYS_TIMES: i64 = 100;
pub const SYS_GETUID: i64 = 102;
pub const SYS_SYSLOG: i64 = 103;
pub const SYS_GETGID: i64 = 104;
pub const SYS_SETUID: i64 = 105;
pub const SYS_SETGID: i64 = 106;
pub const SYS_GETEUID: i64 = 107;
pub const SYS_GETEGID: i64 = 108;
pub const SYS_SETPGID: i64 = 109;
pub const SYS_GETPPID: i64 = 110;
pub const SYS_GETPGRP: i64 = 111;
pub const SYS_SETSID: i64 = 112;
pub const SYS_SETREUID: i64 = 113;
pub const SYS_SETREGID: i64 = 114;
pub const SYS_GETGROUPS: i64 = 115;
pub const SYS_SETGROUPS: i64 = 116;
pub const SYS_SETFSUID: i64 = 122;
pub const SYS_SETFSGID: i64 = 123;
pub const SYS_SETRESUID: i64 = 117;
pub const SYS_GETRESUID: i64 = 118;
pub const SYS_SETRESGID: i64 = 119;
pub const SYS_GETRESGID: i64 = 120;
pub const SYS_GETPGID: i64 = 121;
pub const SYS_GETSID: i64 = 124;
pub const SYS_CAPGET: i64 = 125;
pub const SYS_CAPSET: i64 = 126;
pub const SYS_RT_SIGPENDING: i64 = 127;
pub const SYS_RT_SIGTIMEDWAIT: i64 = 128;
pub const SYS_RT_SIGQUEUEINFO: i64 = 129;
pub const SYS_RT_SIGSUSPEND: i64 = 130;
pub const SYS_SIGALTSTACK: i64 = 131;
pub const SYS_UTIME: i64 = 132;
pub const SYS_MKNOD: i64 = 133;
pub const SYS_PERSONALITY: i64 = 135;
pub const SYS_STATFS: i64 = 137;
pub const SYS_FSTATFS: i64 = 138;
pub const SYS_PIVOT_ROOT: i64 = 155;
pub const SYS_PRCTL: i64 = 157;
pub const SYS_ARCH_PRCTL: i64 = 158;
pub const SYS_CHROOT: i64 = 161;
pub const SYS_MOUNT: i64 = 165;
pub const SYS_UMOUNT2: i64 = 166;
pub const SYS_SWAPON: i64 = 167;
pub const SYS_SWAPOFF: i64 = 168;
pub const SYS_REBOOT: i64 = 169;
pub const SYS_SETHOSTNAME: i64 = 170;
pub const SYS_SETDOMAINNAME: i64 = 171;
pub const SYS_GETTID: i64 = 186;
pub const SYS_SETXATTR: i64 = 188;
pub const SYS_FREMOVEXATTR: i64 = 199;
pub const SYS_FUTEX: i64 = 202;
pub const SYS_SCHED_SETAFFINITY: i64 = 203;
pub const SYS_SCHED_GETAFFINITY: i64 = 204;
pub const SYS_IO_SETUP: i64 = 206;
pub const SYS_IO_DESTROY: i64 = 207;
pub const SYS_IO_GETEVENTS: i64 = 208;
pub const SYS_IO_SUBMIT: i64 = 209;
pub const SYS_IO_CANCEL: i64 = 210;
pub const SYS_GETDENTS64: i64 = 217;
pub const SYS_SET_TID_ADDRESS: i64 = 218;
pub const SYS_CLOCK_SETTIME: i64 = 227;
pub const SYS_CLOCK_GETTIME: i64 = 228;
pub const SYS_CLOCK_GETRES: i64 = 229;
pub const SYS_CLOCK_NANOSLEEP: i64 = 230;
pub const SYS_EXIT_GROUP: i64 = 231;
pub const SYS_EPOLL_WAIT: i64 = 232;
pub const SYS_EPOLL_CTL: i64 = 233;
pub const SYS_TGKILL: i64 = 234;
pub const SYS_UTIMES: i64 = 235;
pub const SYS_OPENAT: i64 = 257;
pub const SYS_WAITID: i64 = 247;
pub const SYS_MKDIRAT: i64 = 258;
pub const SYS_MKNODAT: i64 = 259;
pub const SYS_FCHOWNAT: i64 = 260;
pub const SYS_FUTIMESAT: i64 = 261;
pub const SYS_NEWFSTATAT: i64 = 262;
pub const SYS_UNLINKAT: i64 = 263;
pub const SYS_RENAMEAT: i64 = 264;
pub const SYS_LINKAT: i64 = 265;
pub const SYS_SYMLINKAT: i64 = 266;
pub const SYS_READLINKAT: i64 = 267;
pub const SYS_FCHMODAT: i64 = 268;
pub const SYS_FCHMODAT2: i64 = 452;
pub const SYS_FACCESSAT: i64 = 269;
pub const SYS_FACCESSAT2: i64 = 439;
pub const SYS_PSELECT6: i64 = 270;
pub const SYS_PPOLL: i64 = 271;
pub const SYS_UNSHARE: i64 = 272;
pub const SYS_SET_ROBUST_LIST: i64 = 273;
pub const SYS_GET_ROBUST_LIST: i64 = 274;
pub const SYS_SPLICE: i64 = 275;
pub const SYS_TEE: i64 = 276;
pub const SYS_SYNC_FILE_RANGE: i64 = 277;
pub const SYS_VMSPLICE: i64 = 278;
pub const SYS_UTIMENSAT: i64 = 280;
pub const SYS_EPOLL_PWAIT: i64 = 281;
pub const SYS_SIGNALFD: i64 = 282;
pub const SYS_TIMERFD_CREATE: i64 = 283;
pub const SYS_EVENTFD: i64 = 284;
pub const SYS_FALLOCATE: i64 = 285;
pub const SYS_TIMERFD_SETTIME: i64 = 286;
pub const SYS_TIMERFD_GETTIME: i64 = 287;
pub const SYS_ACCEPT4: i64 = 288;
pub const SYS_SIGNALFD4: i64 = 289;
pub const SYS_EVENTFD2: i64 = 290;
pub const SYS_EPOLL_CREATE1: i64 = 291;
pub const SYS_DUP3: i64 = 292;
pub const SYS_PIPE2: i64 = 293;
pub const SYS_PRLIMIT64: i64 = 302;
pub const SYS_SETNS: i64 = 308;
pub const SYS_SECCOMP: i64 = 317;
pub const SYS_GETRANDOM: i64 = 318;
pub const SYS_MEMFD_CREATE: i64 = 319;
pub const SYS_BPF: i64 = 321;
pub const SYS_EXECVEAT: i64 = 322;
pub const SYS_MEMBARRIER: i64 = 324;
pub const SYS_STATX: i64 = 332;
pub const SYS_SETRLIMIT: i64 = 160;
pub const SYS_INOTIFY_INIT: i64 = 253;
pub const SYS_INOTIFY_ADD_WATCH: i64 = 254;
pub const SYS_INOTIFY_RM_WATCH: i64 = 255;
pub const SYS_INOTIFY_INIT1: i64 = 294;
pub const SYS_NAME_TO_HANDLE_AT: i64 = 303;
pub const SYS_OPEN_BY_HANDLE_AT: i64 = 304;
pub const SYS_COPY_FILE_RANGE: i64 = 326;
pub const SYS_CLOSE_RANGE: i64 = 436;
pub const SYS_OPENAT2: i64 = 437;

static POST_WAIT_TRACE_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub const SYS_EPOLL_PWAIT2: i64 = 441;

/// Converts a libc-convention result into the raw kernel convention.
///
/// The two conventions are not the same and mixing them is the single easiest way
/// to break this gate. A libc function reports failure as `-1` with the code in
/// errno; the kernel reports it as the negated code in the return register and
/// never touches errno. Handing a caller `-1` where `-EPERM` was expected makes
/// the failure look like the successful result `-1`, and handing it `-EPERM` where
/// `-1` was expected makes an error look like a large positive success.
///
/// So every dispatch below funnels through here: a non-negative result passes
/// through, and a `-1` is re-encoded from errno.
fn to_kernel(result: i64) -> i64 {
    if result >= 0 {
        return result;
    }
    let error = kinakaze_tls::errno();
    // A libc function that returned -1 without setting errno leaves nothing to
    // report; EIO is closer to the truth than claiming success.
    if error <= 0 {
        return -i64::from(kinakaze_vfs::EIO);
    }
    -i64::from(error)
}

/// The Linux `struct timespec` as the kernel defines it on x86_64.
///
/// Named for the kernel rather than for C because this module writes it only on
/// the raw `SYS_clock_gettime` path.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KernelTimespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

/// The structure consumed by the x86_64 Linux `rt_sigaction` syscall.
///
/// This is deliberately distinct from glibc's public `struct sigaction`:
/// the kernel mask is one 64-bit word and follows the restorer, while glibc's
/// 128-byte mask precedes the flags. Mixing the two overwrites caller stacks.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KernelSigAction {
    sa_handler: usize,
    sa_flags: u64,
    sa_restorer: usize,
    sa_mask: u64,
}

/// `clockid_t` values, which are ABI.
const CLOCK_REALTIME: i64 = 0;
const CLOCK_MONOTONIC: i64 = 1;
const CLOCK_PROCESS_CPUTIME_ID: i64 = 2;
const CLOCK_THREAD_CPUTIME_ID: i64 = 3;
const CLOCK_MONOTONIC_RAW: i64 = 4;
const CLOCK_REALTIME_COARSE: i64 = 5;
const CLOCK_MONOTONIC_COARSE: i64 = 6;
const CLOCK_BOOTTIME: i64 = 7;

/// 100-nanosecond ticks between the Windows epoch (1601) and the Unix epoch.
///
/// Windows counts from 1601-01-01; Unix from 1970-01-01. The difference is
/// 11644473600 seconds, and a FILETIME tick is 100 ns.
const WINDOWS_TO_UNIX_TICKS: u64 = 11_644_473_600 * 10_000_000;

/// Serves `SYS_clock_gettime` for the clocks Windows can answer.
///
/// `CLOCK_REALTIME` is `GetSystemTimeAsFileTime` rebased on the Unix epoch. The
/// monotonic and boot clocks use unbiased and biased Windows interrupt time,
/// respectively, with the offsets of the current time namespace.
///
/// The CPU-time clocks come from `GetProcessTimes` and `GetThreadTimes`, which are
/// exact: kernel plus user time, the same sum Linux reports.
///
/// # Safety
///
/// `result` must point at a writable `struct timespec`.
unsafe fn clock_gettime(clock: i64, result: *mut KernelTimespec) -> i64 {
    if result.is_null() {
        return -i64::from(EFAULT);
    }
    /// Splits a count of 100 ns ticks into whole seconds and nanoseconds.
    fn from_ticks(ticks: u64) -> KernelTimespec {
        KernelTimespec {
            tv_sec: (ticks / 10_000_000) as i64,
            tv_nsec: ((ticks % 10_000_000) * 100) as i64,
        }
    }

    let value = match clock {
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE | 8 => {
            let mut now = FileTime::default();
            // SAFETY: `now` is a writable local; the call cannot fail.
            unsafe { GetSystemTimeAsFileTime(&raw mut now) };
            from_ticks(now.ticks().saturating_sub(WINDOWS_TO_UNIX_TICKS))
        }
        CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_MONOTONIC_COARSE | CLOCK_BOOTTIME | 9 => {
            match kinakaze_vfs::time_namespace::clock(clock as i32) {
                Ok((tv_sec, tv_nsec)) => KernelTimespec { tv_sec, tv_nsec },
                Err(error) => return -i64::from(error),
            }
        }
        CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            let mut creation = FileTime::default();
            let mut exit = FileTime::default();
            let mut kernel = FileTime::default();
            let mut user = FileTime::default();
            let ok = if clock == CLOCK_PROCESS_CPUTIME_ID {
                // SAFETY: the pseudo-handle is valid and all four out-parameters
                // are writable locals.
                unsafe {
                    GetProcessTimes(
                        GetCurrentProcess(),
                        &raw mut creation,
                        &raw mut exit,
                        &raw mut kernel,
                        &raw mut user,
                    )
                }
            } else {
                // SAFETY: as above, for the thread pseudo-handle.
                unsafe {
                    GetThreadTimes(
                        GetCurrentThread(),
                        &raw mut creation,
                        &raw mut exit,
                        &raw mut kernel,
                        &raw mut user,
                    )
                }
            };
            if ok == 0 {
                // SAFETY: no preconditions.
                return -i64::from(kinakaze_vfs::errno_from_win32(unsafe { GetLastError() }));
            }
            from_ticks(kernel.ticks() + user.ticks())
        }
        // Linux reports EINVAL for a clock it does not know, which includes the
        // per-process and per-thread dynamic clock ids this cannot resolve.
        _ => return -i64::from(EINVAL),
    };
    // SAFETY: the caller guarantees a writable struct and `result` was checked
    // non-null above.
    unsafe { *result = value };
    0
}

/// `GRND_NONBLOCK` and `GRND_RANDOM`, the two flags `getrandom` defines.
const GRND_NONBLOCK: u32 = 0x0001;
const GRND_RANDOM: u32 = 0x0002;

/// Serves `SYS_getrandom` from the system CSPRNG.
///
/// Both flags are accepted and neither changes the behaviour, which is honest on
/// this host. `GRND_NONBLOCK` asks not to block waiting for the entropy pool to
/// initialise; the Windows generator is seeded before user code runs, so there is
/// nothing to block on and the request is trivially satisfied. `GRND_RANDOM` asks
/// for the `/dev/random` pool rather than `/dev/urandom`, a distinction Windows
/// does not draw — and one Linux itself has not drawn since 5.6.
///
/// An unknown flag is `EINVAL`, matching the kernel, so a caller using a newer flag
/// learns it was not honoured instead of assuming it was.
///
/// # Safety
///
/// `buffer` must name at least `length` writable bytes.
unsafe fn getrandom(buffer: *mut c_void, length: usize, flags: u32) -> i64 {
    if flags & !(GRND_NONBLOCK | GRND_RANDOM) != 0 {
        return -i64::from(EINVAL);
    }
    // A zero-length request is well-formed and asks for nothing.
    if length == 0 {
        return 0;
    }
    if buffer.is_null() {
        return -i64::from(EFAULT);
    }
    // The Win32 length is a u32. A larger request is served in one call only up to
    // that; Linux caps a single getrandom at 32 MiB anyway and returns a short
    // count, which is the same shape as this.
    let count = length.min(u32::MAX as usize);
    // SAFETY: the caller guarantees `length` writable bytes and `count` is at most
    // that.
    let ok = unsafe { RtlGenRandom(buffer, count as u32) };
    if ok == 0 {
        // The generator failing is not something a caller can retry usefully, but
        // it must never look like a buffer full of zeros that succeeded.
        return -i64::from(kinakaze_vfs::EIO);
    }
    count as i64
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getrandom(
    buffer: *mut c_void,
    length: usize,
    flags: u32,
) -> isize {
    let result = unsafe { getrandom(buffer, length, flags) };
    if result < 0 {
        set_errno((-result) as i32);
        -1
    } else {
        result as isize
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getentropy(buffer: *mut c_void, length: usize) -> c_int {
    if length > 256 {
        set_errno(kinakaze_vfs::EIO);
        return -1;
    }
    let result = unsafe { getrandom(buffer, length, 0) };
    if result < 0 {
        set_errno((-result) as i32);
        -1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_arc4random() -> u32 {
    let mut val: u32 = 0;
    unsafe { RtlGenRandom((&raw mut val).cast(), 4) };
    val
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getauxval(type_: usize) -> usize {
    match type_ {
        6 => 4096, // AT_PAGESZ
        17 => 100, // AT_CLKTCK
        23 => 0,   // AT_SECURE
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_get_nprocs() -> c_int {
    std::thread::available_parallelism()
        .map(|n| n.get() as c_int)
        .unwrap_or(1)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_gnu_get_libc_version() -> *const c_char {
    c"2.36".as_ptr()
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sched_getcpu() -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___sched_cpucount(
    setsize: usize,
    set: *const u64,
) -> c_int {
    if set.is_null() {
        return 0;
    }
    let words = setsize / 8;
    let mut count = 0;
    for i in 0..words {
        count += unsafe { (*set.add(i)).count_ones() as c_int };
    }
    count
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___sched_cpualloc(count: usize) -> *mut u64 {
    let size = (count + 63) / 64 * 8;
    let layout = std::alloc::Layout::from_size_align(size, 8).unwrap();
    unsafe { std::alloc::alloc_zeroed(layout).cast() }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___sched_cpufree(ptr: *mut u64) {
    if !ptr.is_null() {
        // free
    }
}

/// `membarrier` commands, which are ABI.
const MEMBARRIER_CMD_QUERY: i64 = 0;
const MEMBARRIER_CMD_GLOBAL: i64 = 1 << 0;
const MEMBARRIER_CMD_GLOBAL_EXPEDITED: i64 = 1 << 1;
const MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED: i64 = 1 << 2;
const MEMBARRIER_CMD_PRIVATE_EXPEDITED: i64 = 1 << 3;
const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED: i64 = 1 << 4;

/// Serves `SYS_membarrier`.
///
/// `FlushProcessWriteBuffers` is the genuine equivalent of the barrier commands:
/// it interrupts every processor currently running one of this process's threads
/// and drains its store buffer, which is the same mechanism and the same guarantee
/// the kernel provides. This is what the Windows implementations of
/// `std::atomic_thread_fence(memory_order_seq_cst)`-style process-wide fences and
/// of asymmetric reclamation use.
///
/// The `QUERY` command reports the commands supported as a bitmask, which is how a
/// caller discovers the facility. Only the commands actually served are advertised,
/// so a caller that checks the mask — which is the documented way to use this — is
/// never told about a command that would then fail.
///
/// The two `REGISTER_*` commands succeed without doing anything, and that is
/// correct rather than a shortcut: registration exists so the kernel knows which
/// processes to interrupt for the expedited variants, and
/// `FlushProcessWriteBuffers` needs no such bookkeeping.
fn membarrier(command: i64, flags: u32) -> i64 {
    // No flag is defined for any command except the cpu-id form, which is not
    // served here, so a non-zero flags word is EINVAL as the kernel reports.
    if flags != 0 {
        return -i64::from(EINVAL);
    }
    match command {
        MEMBARRIER_CMD_QUERY => {
            MEMBARRIER_CMD_GLOBAL
                | MEMBARRIER_CMD_GLOBAL_EXPEDITED
                | MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED
                | MEMBARRIER_CMD_PRIVATE_EXPEDITED
                | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED
        }
        MEMBARRIER_CMD_GLOBAL
        | MEMBARRIER_CMD_GLOBAL_EXPEDITED
        | MEMBARRIER_CMD_PRIVATE_EXPEDITED => {
            // SAFETY: no arguments and no preconditions.
            unsafe { FlushProcessWriteBuffers() };
            0
        }
        MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED => 0,
        _ => -i64::from(EINVAL),
    }
}

/// `syscall`, the raw kernel gate.
///
/// Variadic in C, and fixed-arity here for the reason [`kinakaze_abi_prctl`]
/// explains — but with one limit worth stating exactly, because the arithmetic is
/// tighter than it looks.
///
/// A Linux syscall takes up to six arguments, and `syscall` prepends the number,
/// so a fully loaded call passes *seven* values. System V has six integer argument
/// registers. The seventh — a syscall's sixth argument — therefore lands on the
/// stack, and this signature does not name it: five arguments are declared, which
/// covers the number plus five. No syscall dispatched below takes more than four,
/// so nothing served here is affected, and a caller invoking a six-argument
/// syscall reaches the `-ENOSYS` default rather than being served with a truncated
/// argument list. Adding one would mean declaring the sixth parameter so the
/// compiler loads it from the caller's frame.
///
/// A caller passing fewer arguments leaves the tail registers holding stale
/// values, so each dispatch reads only the arguments its number is defined to
/// take.
///
/// # The return convention, which is not errno
///
/// This returns what the kernel returns: a non-negative result on success, or the
/// **negated errno as the return value** on failure, with errno itself untouched.
/// `syscall(SYS_mount, ...)` yields `-38`, not `-1`. glibc's `syscall` wrapper is
/// what converts that into `-1` plus errno for its caller, and getting the
/// direction wrong here makes every error indistinguishable from a success: a
/// caller checking `< 0` would see `-38` as an error but a caller checking `== -1`
/// would see it as a valid result, and vice versa.
///
/// # Numbers served
///
/// Real work:
///
/// - `SYS_gettid` (186) — the Windows thread id. See [`current_tid`].
/// - `SYS_getpid` (39) — the process id, dispatched because a `gettid` caller on a
///   single-threaded guest often probes both.
/// - `SYS_getrandom` (318) — the system CSPRNG, via `RtlGenRandom`.
/// - `SYS_membarrier` (324) — `FlushProcessWriteBuffers`, including the `QUERY`
///   form.
/// - `SYS_sched_getaffinity` (204) and `SYS_sched_setaffinity` (203) — the Windows
///   affinity mask. Note the return value: the raw `sched_getaffinity` reports the
///   number of bytes written, where the glibc wrapper reports 0.
/// - `SYS_sched_yield` (24) — `SwitchToThread`.
/// - `SYS_clock_gettime` (228) — the Windows clocks; see [`clock_gettime`].
/// - `SYS_ioctl` (16) — forwarded to this crate's `ioctl`, which serves the
///   terminal and descriptor requests and reports `ENOTTY` for the rest.
/// - `SYS_readlinkat` (267) — forwarded to this crate's `readlinkat`.
/// - `SYS_set_tid_address` (218) — see [`set_tid_address`] for what it does and
///   does not promise.
/// - `SYS_prctl` (157) and `SYS_personality` (135) — the implementations above.
///
/// Truthful refusals, dispatched explicitly rather than left to fall through, so
/// the raw path and the wrapper report the same errno for the same request:
/// `SYS_mount`, `SYS_umount2`, `SYS_swapon`, `SYS_swapoff`, `SYS_pivot_root`,
/// `SYS_unshare`, `SYS_setns`, `SYS_chroot`, `SYS_syslog` and `SYS_reboot`. The
/// reboot path is the one place the magic numbers are checked, because it is the
/// only path they are passed on.
///
/// `SYS_futex` (202) implements process-private wait, wake, bitsets and compare
/// requeue, including unflagged operations on private memory. Futexes on shared
/// mappings require a cross-process queue and return `-ENOSYS` until it exists.
///
/// **Every other number returns `-ENOSYS`** — negative, per the convention above.
/// That is what Linux returns for a syscall the kernel does not implement, so a
/// caller with a fallback path takes it.
///
/// # Safety
///
/// Any pointer among the arguments must satisfy whatever the dispatched syscall
/// requires of it. An unrecognised number dereferences nothing.
const FUTEX_WAIT: u32 = 0;
const FUTEX_WAKE: u32 = 1;
const FUTEX_REQUEUE: u32 = 3;
const FUTEX_CMP_REQUEUE: u32 = 4;
const FUTEX_WAKE_OP: u32 = 5;
const FUTEX_WAIT_BITSET: u32 = 9;
const FUTEX_WAKE_BITSET: u32 = 10;
const FUTEX_PRIVATE_FLAG: u32 = 128;
const FUTEX_CLOCK_REALTIME: u32 = 256;

const FUTEX_BITSET_MATCH_ANY: u32 = u32::MAX;

/// One kernel-style sleep record. The address can change while the waiter is
/// requeued, so timeout removal follows `address` rather than assuming the
/// original queue still owns it.
struct FutexWaiter {
    address: AtomicUsize,
    bitset: u32,
    thread: u32,
}

type FutexQueue = HashMap<usize, VecDeque<Arc<FutexWaiter>>>;

#[derive(Clone, Copy)]
struct FutexAddress {
    word: *mut c_int,
    key: usize,
    shared: Option<crate::futex::Key>,
}

impl From<*mut c_int> for FutexAddress {
    fn from(word: *mut c_int) -> Self {
        Self {
            word,
            key: word as usize,
            shared: None,
        }
    }
}

impl FutexAddress {
    fn resolve(word: *mut c_int, private: bool) -> Result<Self, i64> {
        futex_word(word)?;
        // All unflagged keys use the kernel-domain queue, even for private
        // memory, so mixed private/shared requeues remain atomic. PRIVATE_FLAG
        // retains the process-local fast path and its separate key namespace.
        Ok(Self {
            word,
            key: word as usize | usize::from(!private),
            shared: if private {
                None
            } else {
                Some(crate::fdio::futex_key(word as usize).map_err(|error| -i64::from(error))?)
            },
        })
    }
}

// A fork child has no inherited kernel waiters. Keep the process-local queue
// behind a replaceable pointer so the registered child hook never locks or
// drops a parent allocation (including a mutex held by a vanished sibling).
static FUTEX_QUEUES: AtomicPtr<Mutex<FutexQueue>> = AtomicPtr::new(core::ptr::null_mut());

fn futex_queues() -> &'static Mutex<FutexQueue> {
    let mut queues = FUTEX_QUEUES.load(Ordering::Acquire);
    if queues.is_null() {
        let candidate = Box::into_raw(Box::new(Mutex::new(HashMap::new())));
        queues = match FUTEX_QUEUES.compare_exchange(
            core::ptr::null_mut(),
            candidate,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => candidate,
            Err(existing) => {
                // SAFETY: publication failed, so only this thread owns it.
                unsafe { drop(Box::from_raw(candidate)) };
                existing
            }
        };
    }
    // SAFETY: published queues live for this process's lifetime. The child hook
    // resets only the child's pointer, before any guest thread resumes there.
    unsafe { &*queues }
}

#[cfg(windows)]
mod futex_handoff {
    unsafe extern "system" fn child(_: *const u8, len: usize) -> i32 {
        if len != 0 {
            return kinakaze_vfs::EINVAL;
        }
        super::FUTEX_QUEUES.store(core::ptr::null_mut(), super::Ordering::Release);
        crate::futex::reset_after_fork();
        0
    }

    extern "C" fn initializer() {
        assert!(kinakaze_runtime::register_fork_participant(
            kinakaze_runtime::ForkParticipant {
                abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
                priority: 45,
                key: 0x4c49_4243_4655_5431, // "LIBCFUT1"
                prepare: None,
                snapshot: None,
                parent: None,
                child: Some(child),
            }
        ));
    }

    #[used]
    #[unsafe(link_section = ".CRT$XCU")]
    static INITIALIZER: extern "C" fn() = initializer;
}

fn futex_word(address: *mut c_int) -> Result<&'static AtomicI32, i64> {
    if address.is_null() {
        return Err(-i64::from(EFAULT));
    }
    if (address as usize) & (core::mem::align_of::<i32>() - 1) != 0 {
        return Err(-i64::from(EINVAL));
    }
    futex_access(address as usize, size_of::<i32>(), false)?;
    // SAFETY: a Linux futex word is a naturally aligned, process-shared i32.
    // Its storage is owned by the guest and must outlive every wait on it.
    Ok(unsafe { &*(address.cast::<AtomicI32>()) })
}

fn futex_access(address: usize, length: usize, write: bool) -> Result<(), i64> {
    use windows_sys::Win32::System::Memory::*;
    let end = address.checked_add(length).ok_or(-i64::from(EFAULT))?;
    let mut cursor = address;
    while cursor < end {
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        if unsafe {
            VirtualQuery(
                cursor as _,
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
            || info.State != MEM_COMMIT
            || info.Protect & PAGE_GUARD != 0
            || !matches!(
                info.Protect & 0xff,
                PAGE_READONLY
                    | PAGE_READWRITE
                    | PAGE_WRITECOPY
                    | PAGE_EXECUTE_READ
                    | PAGE_EXECUTE_READWRITE
                    | PAGE_EXECUTE_WRITECOPY
            )
            || (write
                && !matches!(
                    info.Protect & 0xff,
                    PAGE_READWRITE
                        | PAGE_WRITECOPY
                        | PAGE_EXECUTE_READWRITE
                        | PAGE_EXECUTE_WRITECOPY
                ))
        {
            return Err(-i64::from(EFAULT));
        }
        let next = (info.BaseAddress as usize).saturating_add(info.RegionSize);
        if next <= cursor {
            return Err(-i64::from(EFAULT));
        }
        cursor = next;
    }
    Ok(())
}

fn timespec_ns(value: &KernelTimespec) -> Result<u128, i64> {
    if value.tv_sec < 0 || value.tv_nsec < 0 || value.tv_nsec >= 1_000_000_000 {
        return Err(-i64::from(EINVAL));
    }
    Ok((value.tv_sec as u128)
        .saturating_mul(1_000_000_000)
        .saturating_add(value.tv_nsec as u128))
}

fn futex_timeout(
    timeout: *const KernelTimespec,
    absolute: bool,
    realtime: bool,
) -> Result<Option<std::time::Duration>, i64> {
    if timeout.is_null() {
        return Ok(None);
    }
    futex_access(timeout as usize, size_of::<KernelTimespec>(), false)?;
    // SAFETY: the syscall contract supplies a readable kernel timespec.
    let target = timespec_ns(unsafe { &*timeout })?;
    let remaining = if absolute {
        let mut now = KernelTimespec::default();
        // SAFETY: `now` is a writable local and the selected clock is supported.
        let result = unsafe {
            clock_gettime(
                if realtime {
                    CLOCK_REALTIME
                } else {
                    CLOCK_MONOTONIC
                },
                &raw mut now,
            )
        };
        if result < 0 {
            return Err(result);
        }
        let now = timespec_ns(&now)?;
        // The value comparison precedes expiry in Linux futex_wait_setup:
        // an already expired timeout does not override EAGAIN.
        target.saturating_sub(now)
    } else {
        target
    };
    let seconds = (remaining / 1_000_000_000).min(u128::from(u64::MAX)) as u64;
    let nanos = (remaining % 1_000_000_000) as u32;
    Ok(Some(std::time::Duration::new(seconds, nanos)))
}

fn remove_empty_futex_queue(queues: &mut FutexQueue, address: usize) {
    if queues.get(&address).is_some_and(VecDeque::is_empty) {
        queues.remove(&address);
    }
}

fn select_futex_wake(queues: &mut FutexQueue, address: usize, count: u32, bitset: u32) -> usize {
    if count == 0 {
        return 0;
    }
    let mut selected = 0;
    if let Some(queue) = queues.get_mut(&address) {
        let mut index = 0;
        while index < queue.len() && selected < count as usize {
            if queue[index].bitset & bitset != 0 {
                if let Some(waiter) = queue.remove(index) {
                    // Publish the wake before dropping the queue lock. A
                    // timeout/signal can then observe removal as success, with
                    // no second uninterruptible wait for a waker to publish.
                    kinakaze_vfs::interrupt::interrupt_thread(waiter.thread);
                    selected += 1;
                }
            } else {
                index += 1;
            }
        }
    }
    remove_empty_futex_queue(queues, address);
    selected
}

fn futex_wake(address: impl Into<FutexAddress>, count: u32, bitset: u32) -> i64 {
    let address = address.into();
    if let Err(error) = futex_word(address.word) {
        return error;
    }
    if bitset == 0 {
        return -i64::from(EINVAL);
    }
    // The legacy wake ABI checks its signed limit after the first selection.
    let count = (count as i32).max(1) as u32;
    if let Some(key) = address.shared {
        return crate::futex::wake(key, count, bitset)
            .map_or_else(|error| -i64::from(error), i64::from);
    }
    let waiters = {
        let mut queues = futex_queues()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        select_futex_wake(&mut queues, address.key, count, bitset)
    };
    waiters as i64
}

/// Apply the encoded operation and select both wake sets under the same queue
/// lock used by FUTEX_WAIT's value check. This also handles identical addresses
/// without waking a waiter twice. Atomic RMWs remain necessary because guest
/// instructions modify the words without taking this lock.
fn futex_wake_op(
    address: FutexAddress,
    count: u32,
    address2: FutexAddress,
    count2: u32,
    encoded: u32,
) -> i64 {
    let word = match futex_word(address2.word) {
        Ok(word) => word,
        Err(error) => return error,
    };
    if let Err(error) = futex_access(address2.word as usize, size_of::<i32>(), true) {
        return error;
    }
    if let (Some(key), Some(destination)) = (address.shared, address2.shared) {
        return crate::futex::wake_op(
            key,
            (count as i32).max(1) as u32,
            destination,
            (count2 as i32).max(1) as u32,
            || futex_atomic_op(word, encoded).map_err(|error| -error as i32),
        )
        .map_or_else(|error| -i64::from(error), i64::from);
    }
    let mut queues = futex_queues()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let wake_second = match futex_atomic_op(word, encoded) {
        Ok(second) => second,
        Err(error) => return error,
    };
    let mut woken = select_futex_wake(
        &mut queues,
        address.key,
        (count as i32).max(1) as u32,
        FUTEX_BITSET_MATCH_ANY,
    );
    if wake_second {
        woken += select_futex_wake(
            &mut queues,
            address2.key,
            (count2 as i32).max(1) as u32,
            FUTEX_BITSET_MATCH_ANY,
        );
    }
    woken as i64
}

fn futex_atomic_op(word: &AtomicI32, encoded: u32) -> Result<bool, i64> {
    let operation = (encoded >> 28) & 7;
    // Both operands are signed twelve-bit integers in the Linux ABI.
    let mut operand = ((encoded << 8) as i32) >> 20;
    let comparison = ((encoded << 20) as i32) >> 20;
    if encoded & (1 << 31) != 0 {
        // Linux/x86 masks even an out-of-range shift to five bits.
        operand = 1i32.wrapping_shl(operand as u32);
    }
    let old = match operation {
        0 => word.swap(operand, Ordering::SeqCst),
        1 => word.fetch_add(operand, Ordering::SeqCst),
        2 => word.fetch_or(operand, Ordering::SeqCst),
        3 => word.fetch_and(!operand, Ordering::SeqCst),
        4 => word.fetch_xor(operand, Ordering::SeqCst),
        _ => return Err(-i64::from(ENOSYS)),
    };
    // Compare the value before the operation, including its sign. Linux checks
    // an invalid comparison after the RMW, but does not wake either queue.
    Ok(match (encoded >> 24) & 15 {
        0 => old == comparison,
        1 => old != comparison,
        2 => old < comparison,
        3 => old <= comparison,
        4 => old > comparison,
        5 => old >= comparison,
        _ => return Err(-i64::from(ENOSYS)),
    })
}

/// Returns false if a concurrent wake already removed this sleep record.
fn unqueue_futex(waiter: &Arc<FutexWaiter>) -> bool {
    let mut queues = futex_queues()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let current = waiter.address.load(Ordering::Acquire);
    let removed = queues.get_mut(&current).is_some_and(|queue| {
        queue
            .iter()
            .position(|candidate| Arc::ptr_eq(candidate, waiter))
            .and_then(|index| queue.remove(index))
            .is_some()
    });
    remove_empty_futex_queue(&mut queues, current);
    removed
}

fn futex_wait(
    address: impl Into<FutexAddress>,
    expected: i32,
    timeout: *const KernelTimespec,
    absolute: bool,
    realtime: bool,
    bitset: u32,
) -> i64 {
    use kinakaze_vfs::{EINTR, EIO, interrupt, signal};
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};

    let address = address.into();
    let word = match futex_word(address.word) {
        Ok(word) => word,
        Err(error) => return error,
    };
    if bitset == 0 {
        return -i64::from(EINVAL);
    }
    let duration = match futex_timeout(timeout, absolute, realtime) {
        Ok(duration) => duration,
        Err(error) => return error,
    };
    if address.shared.is_some() {
        return crate::futex::wait(expected, duration, bitset, || {
            let key = crate::fdio::futex_key(address.word as usize)?;
            let word = futex_word(address.word).map_err(|error| -error as i32)?;
            Ok((key, word.load(Ordering::SeqCst)))
        })
        .map_or_else(|error| -i64::from(error), |()| 0);
    }
    let started = std::time::Instant::now();
    // Reuse the thread's event for both futex wake and signals. The queue is
    // authoritative about which happened; no kernel event is allocated per
    // futex wait. Create it before publication so a wake cannot be lost.
    let event = interrupt::current();
    if event.is_null() {
        return -i64::from(EIO);
    }
    let waiter = Arc::new(FutexWaiter {
        address: AtomicUsize::new(address.key),
        bitset,
        thread: interrupt::current_thread_id(),
    });
    loop {
        let mut queues = futex_queues()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Registration and the second value check share the wake-side lock. A
        // concurrent wake therefore sees us or we see the changed word; there is
        // no interval in which both operations can miss each other.
        if word.load(Ordering::SeqCst) != expected {
            return -i64::from(EAGAIN);
        }
        queues
            .entry(address.key)
            .or_default()
            .push_back(Arc::clone(&waiter));
        waiter.address.store(address.key, Ordering::Release);
        drop(queues);

        signal::register_waiter();
        // Check after registration, closing the signal-before-park window.
        // Never run a guest handler while its outer futex remains queued:
        // handlers may themselves wait, wake or fork.
        let pending = signal::pending() & !signal::blocked_mask() != 0;
        let remaining = duration.map(|limit| limit.saturating_sub(started.elapsed()));
        let milliseconds = remaining.map_or(INFINITE, |left| {
            left.as_nanos()
                .div_ceil(1_000_000)
                .min(u128::from(INFINITE - 1)) as u32
        });
        let status = if pending {
            WAIT_OBJECT_0
        } else {
            // SAFETY: the event belongs to the calling thread for its lifetime.
            unsafe { WaitForSingleObject(event, milliseconds) }
        };
        let removed = unqueue_futex(&waiter);

        signal::unregister_waiter();
        let expired = duration.is_some_and(|limit| started.elapsed() >= limit);
        let delivery = signal::deliver_pending();
        // As in Linux __futex_wait, a selected wake wins over timeout/signal.
        if !removed {
            return 0;
        }
        if status != WAIT_OBJECT_0 && status != WAIT_TIMEOUT {
            return -i64::from(EIO);
        }
        if expired {
            return -i64::from(ETIMEDOUT);
        }
        if delivery == signal::Delivery::Interrupted {
            return -i64::from(EINTR);
        }
        // Spurious interrupts and SA_RESTART repeat the value comparison on
        // the original address, retaining the original timeout budget.
    }
}

fn futex_syscall(
    uaddr: *mut c_int,
    op: i32,
    val: u32,
    timeout: *const KernelTimespec,
    uaddr2: *mut c_int,
    val3: u32,
) -> i64 {
    let flags = (op as u32) & (FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
    let cmd = (op as u32) & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
    if flags & FUTEX_CLOCK_REALTIME != 0 && cmd != FUTEX_WAIT_BITSET {
        return -i64::from(ENOSYS);
    }
    if !matches!(
        cmd,
        FUTEX_WAIT
            | FUTEX_WAKE
            | FUTEX_WAIT_BITSET
            | FUTEX_WAKE_BITSET
            | FUTEX_REQUEUE
            | FUTEX_CMP_REQUEUE
            | FUTEX_WAKE_OP
    ) {
        return -i64::from(ENOSYS);
    }
    let private = flags & FUTEX_PRIVATE_FLAG != 0;
    let address = match FutexAddress::resolve(uaddr, private) {
        Ok(address) => address,
        Err(error) => return error,
    };
    match cmd {
        FUTEX_WAIT => futex_wait(
            address,
            val as i32,
            timeout,
            false,
            false,
            FUTEX_BITSET_MATCH_ANY,
        ),
        FUTEX_WAIT_BITSET => futex_wait(
            address,
            val as i32,
            timeout,
            true,
            flags & FUTEX_CLOCK_REALTIME != 0,
            val3,
        ),
        FUTEX_WAKE => futex_wake(address, val, FUTEX_BITSET_MATCH_ANY),
        FUTEX_WAKE_BITSET => futex_wake(address, val, val3),
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            let requeue_count = timeout as usize as u32;
            if (val as i32) < 0 || (requeue_count as i32) < 0 {
                return -i64::from(EINVAL);
            }
            let destination = match FutexAddress::resolve(uaddr2, private) {
                Ok(address) => address,
                Err(error) => return error,
            };
            let word = match futex_word(uaddr) {
                Ok(word) => word,
                Err(error) => return error,
            };
            if let Err(error) = futex_word(uaddr2) {
                return error;
            }
            if let (Some(key), Some(target)) = (address.shared, destination.shared) {
                return crate::futex::requeue(
                    key,
                    target,
                    val,
                    requeue_count,
                    (cmd == FUTEX_CMP_REQUEUE).then_some((word, val3 as i32)),
                )
                .map_or_else(|error| -i64::from(error), i64::from);
            }
            let (woken, moved) = {
                let mut queues = futex_queues()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if cmd == FUTEX_CMP_REQUEUE && word.load(Ordering::SeqCst) != val3 as i32 {
                    return -i64::from(EAGAIN);
                }
                let selected =
                    select_futex_wake(&mut queues, address.key, val, FUTEX_BITSET_MATCH_ANY);
                let mut moved = 0i64;
                let mut transfers = VecDeque::new();
                if let Some(source) = queues.get_mut(&address.key) {
                    while moved < i64::from(requeue_count) {
                        let Some(waiter) = source.pop_front() else {
                            break;
                        };
                        waiter.address.store(destination.key, Ordering::Release);
                        transfers.push_back(waiter);
                        moved += 1;
                    }
                }
                remove_empty_futex_queue(&mut queues, address.key);
                queues
                    .entry(destination.key)
                    .or_default()
                    .append(&mut transfers);
                (selected, moved)
            };
            woken as i64 + moved
        }
        FUTEX_WAKE_OP => {
            let destination = match FutexAddress::resolve(uaddr2, private) {
                Ok(address) => address,
                Err(error) => return error,
            };
            futex_wake_op(address, val, destination, timeout as usize as u32, val3)
        }
        _ => -i64::from(ENOSYS),
    }
}

/// The timer registry and implementation are owned by librt.
unsafe fn runtime_posix_timer_syscall(number: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    let Some(entry) = kinakaze_runtime::services::timers() else {
        return -i64::from(ENOSYS);
    };
    unsafe { entry(number, a1, a2, a3, a4) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_syscall_raw(
    number: i64,
    argument1: u64,
    argument2: u64,
    argument3: u64,
    argument4: u64,
    argument5: u64,
    argument6: u64,
) -> i64 {
    // After exec, only the host wait wrapper survives. A sibling from the old
    // image must not perform another syscall or re-register as a guest process.
    if kinakaze_vfs::job::exec_retired() {
        unsafe { windows_sys::Win32::System::Threading::ExitThread(0) };
    }
    let post_wait_trace = std::env::var_os("KINAKAZE_POST_WAIT_TRACE").is_some()
        && POST_WAIT_TRACE_ACTIVE.load(std::sync::atomic::Ordering::Acquire);
    if post_wait_trace {
        eprintln!(
            "kinakaze: [POST-WAIT ENTER] nr={number} a1={argument1:#x} a2={argument2:#x} a3={argument3:#x}"
        );
    }
    if crate::fork_trace_enabled() {
        #[cfg(all(windows, target_arch = "x86_64"))]
        let incoming_r14: usize;
        #[cfg(all(windows, target_arch = "x86_64"))]
        unsafe {
            core::arch::asm!(
                "mov {value}, r14",
                value = out(reg) incoming_r14,
                options(nomem, nostack, preserves_flags),
            );
        }
        #[cfg(not(all(windows, target_arch = "x86_64")))]
        let incoming_r14 = 0usize;
        let guest_tp = crate::process_thread_pointer();
        let guest_tls_g = if guest_tp >= core::mem::size_of::<usize>() {
            unsafe { ((guest_tp - core::mem::size_of::<usize>()) as *const usize).read_unaligned() }
        } else {
            0
        };
        eprintln!(
            "kinakaze syscall: pid {} num={} a1={:#x} a2={:#x} a3={:#x} guest_r14={incoming_r14:#x} guest_tp={guest_tp:#x} guest_tls_g={guest_tls_g:#x}",
            std::process::id(),
            number,
            argument1,
            argument2,
            argument3
        );
    }
    let res = match number {
        187 => kinakaze_vfs::fs::readahead(argument1 as i32, argument2 as i64, argument3 as usize)
            .map_or_else(|error| -i64::from(error), |()| 0),
        310 | 311 => crate::process_memory::transfer(
            argument1 as i32,
            argument2 as usize,
            argument3 as usize,
            argument4 as usize,
            argument5 as usize,
            argument6 as usize,
            number == 311,
        )
        .map_or_else(|error| -i64::from(error), |count| count as i64),
        222..=226 => unsafe {
            runtime_posix_timer_syscall(number as u64, argument1, argument2, argument3, argument4)
        },
        240..=245 => mqueue::syscall(
            number as u64,
            argument1,
            argument2,
            argument3,
            argument4,
            argument5,
        )
        .unwrap_or_else(|e| -(e as i64)),
        248..=250 => keys::syscall(
            number as u64,
            argument1,
            argument2,
            argument3,
            argument4,
            argument5,
        )
        .unwrap_or_else(|e| -(e as i64)),
        SYS_READ => {
            if argument2 == 0 && argument3 > 0 {
                return -i64::from(EFAULT);
            }
            let slice = unsafe {
                core::slice::from_raw_parts_mut(argument2 as *mut u8, argument3 as usize)
            };
            match kinakaze_vfs::read(argument1 as c_int, slice) {
                Ok(n) => n as i64,
                Err(e) => -i64::from(e),
            }
        }
        SYS_WRITE => {
            if argument2 == 0 && argument3 > 0 {
                return -i64::from(EFAULT);
            }
            let slice =
                unsafe { core::slice::from_raw_parts(argument2 as *const u8, argument3 as usize) };
            match kinakaze_vfs::write(argument1 as c_int, slice) {
                Ok(n) => n as i64,
                Err(e) => -i64::from(e),
            }
        }
        SYS_OPEN => {
            let res = unsafe {
                crate::fs::open(
                    argument1 as *const c_char,
                    argument2 as c_int,
                    argument3 as u32,
                )
            };
            to_kernel(res as i64)
        }
        SYS_CLOSE => match kinakaze_vfs::close(argument1 as c_int) {
            Ok(()) => 0,
            Err(e) => -i64::from(e),
        },
        SYS_STAT => {
            let res = unsafe {
                crate::fs::stat(
                    argument1 as *const c_char,
                    argument2 as *mut kinakaze_vfs::fs::Stat,
                )
            };
            to_kernel(res as i64)
        }
        SYS_FSTAT => {
            let res = unsafe {
                crate::fs::fstat(argument1 as c_int, argument2 as *mut kinakaze_vfs::fs::Stat)
            };
            to_kernel(res as i64)
        }
        SYS_LSTAT => {
            let res = unsafe {
                crate::fs::lstat(
                    argument1 as *const c_char,
                    argument2 as *mut kinakaze_vfs::fs::Stat,
                )
            };
            to_kernel(res as i64)
        }
        SYS_POLL => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_poll(
                    argument1 as *mut crate::fdio::PollFd,
                    argument2 as u64,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_LSEEK => {
            let res = unsafe {
                crate::fs::lseek(argument1 as c_int, argument2 as i64, argument3 as c_int)
            };
            to_kernel(res)
        }
        SYS_MMAP => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_mmap64(
                    argument1 as *mut c_void,
                    argument2 as usize,
                    argument3 as c_int,
                    argument4 as c_int,
                    argument5 as c_int,
                    argument6 as i64,
                )
            };
            if res as isize == -1 {
                to_kernel(-1)
            } else {
                res as usize as i64
            }
        }
        SYS_MPROTECT => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_mprotect(
                    argument1 as *mut c_void,
                    argument2 as usize,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_MUNMAP => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_munmap(argument1 as *mut c_void, argument2 as usize)
            };
            to_kernel(res as i64)
        }
        SYS_STATFS => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_statfs(
                    argument1 as *const c_char,
                    argument2 as *mut crate::fsextra::Statfs,
                )
            };
            to_kernel(res as i64)
        }
        SYS_FSTATFS => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_fstatfs(
                    argument1 as c_int,
                    argument2 as *mut crate::fsextra::Statfs,
                )
            };
            to_kernel(res as i64)
        }
        SYS_RT_SIGACTION => {
            if argument4 != size_of::<u64>() as u64 {
                -i64::from(EINVAL)
            } else {
                let requested = if argument2 == 0 {
                    None
                } else {
                    let kact = unsafe { &*(argument2 as *const KernelSigAction) };
                    let flags = kact.sa_flags as i32;
                    Some(kinakaze_vfs::signal::Action {
                        disposition: crate::signal::disposition_of(kact.sa_handler, flags),
                        flags,
                        mask: kact.sa_mask,
                        restorer: kact.sa_restorer,
                    })
                };
                match kinakaze_vfs::signal::sigaction(argument1 as i32, requested) {
                    Ok(previous) => {
                        if argument3 != 0 {
                            unsafe {
                                core::ptr::write(
                                    argument3 as *mut KernelSigAction,
                                    KernelSigAction {
                                        sa_handler: crate::signal::handler_of(previous.disposition),
                                        sa_flags: previous.flags as u64,
                                        sa_restorer: previous.restorer,
                                        sa_mask: previous.mask,
                                    },
                                );
                            }
                        }
                        0
                    }
                    Err(e) => -(e as i64),
                }
            }
        }
        SYS_RT_SIGPROCMASK => {
            if argument4 != size_of::<u64>() as u64 {
                -i64::from(EINVAL)
            } else if argument2 == 0 {
                if argument3 != 0 {
                    unsafe {
                        core::ptr::write(
                            argument3 as *mut u64,
                            kinakaze_vfs::signal::blocked_mask(),
                        )
                    };
                }
                0
            } else {
                let requested = unsafe { core::ptr::read(argument2 as *const u64) };
                match kinakaze_vfs::signal::sigprocmask(argument1 as i32, requested) {
                    Ok(previous) => {
                        if argument3 != 0 {
                            unsafe { core::ptr::write(argument3 as *mut u64, previous) };
                        }
                        // Unblocking must run a pending handler before returning
                        // to guest code, including a queued SIGSETXID rendezvous.
                        kinakaze_vfs::signal::deliver_pending();
                        0
                    }
                    Err(error) => -i64::from(error),
                }
            }
        }
        SYS_IOCTL => {
            let result = unsafe {
                crate::term::kinakaze_abi_ioctl(
                    argument1 as c_int,
                    argument2,
                    argument3 as *mut c_void,
                )
            };
            to_kernel(i64::from(result))
        }
        SYS_PREAD64 => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_pread64(
                    argument1 as c_int,
                    argument2 as *mut c_void,
                    argument3 as usize,
                    argument4 as i64,
                )
            };
            to_kernel(res as i64)
        }
        SYS_PWRITE64 => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_pwrite64(
                    argument1 as c_int,
                    argument2 as *const c_void,
                    argument3 as usize,
                    argument4 as i64,
                )
            };
            to_kernel(res as i64)
        }
        SYS_READV => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_readv(
                    argument1 as c_int,
                    argument2 as *const crate::fdio::IoVec,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_WRITEV => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_writev(
                    argument1 as c_int,
                    argument2 as *const crate::fdio::IoVec,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_ACCESS => {
            let res = unsafe { crate::fs::access(argument1 as *const c_char, argument2 as c_int) };
            to_kernel(res as i64)
        }
        SYS_PIPE => {
            let res = unsafe { crate::fdio::kinakaze_abi_pipe(argument1 as *mut c_int) };
            to_kernel(res as i64)
        }
        SYS_SELECT => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_select(
                    argument1 as c_int,
                    argument2 as *mut crate::fdio::FdSet,
                    argument3 as *mut crate::fdio::FdSet,
                    argument4 as *mut crate::fdio::FdSet,
                    argument5 as *mut crate::fdio::Timeval,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SCHED_YIELD => i64::from(kinakaze_abi_sched_yield()),
        SYS_MADVISE => {
            let res = crate::fdio::madvise_impl(
                argument1 as *mut c_void,
                argument2 as usize,
                argument3 as c_int,
            );
            match res {
                Ok(()) => 0,
                Err(error) => -i64::from(error),
            }
        }
        SYS_SHMGET => {
            let res = unsafe {
                crate::sysvipc::kinakaze_abi_shmget(
                    argument1 as i32,
                    argument2 as usize,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SHMAT => {
            let res = unsafe {
                crate::sysvipc::kinakaze_abi_shmat(
                    argument1 as c_int,
                    argument2 as *const c_void,
                    argument3 as c_int,
                )
            };
            if res as isize == -1 {
                to_kernel(-1)
            } else {
                res as usize as i64
            }
        }
        SYS_SHMCTL => {
            let res = unsafe {
                crate::sysvipc::kinakaze_abi_shmctl(
                    argument1 as c_int,
                    argument2 as c_int,
                    argument3 as *mut crate::sysvipc::ShmidDs,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SHMDT => {
            let res = unsafe { crate::sysvipc::kinakaze_abi_shmdt(argument1 as *const c_void) };
            to_kernel(res as i64)
        }
        SYS_DUP => {
            let res = unsafe { crate::fdio::kinakaze_abi_dup(argument1 as c_int) };
            to_kernel(res as i64)
        }
        SYS_DUP2 => {
            let res =
                unsafe { crate::fdio::kinakaze_abi_dup2(argument1 as c_int, argument2 as c_int) };
            to_kernel(res as i64)
        }
        SYS_NANOSLEEP => {
            let res = unsafe {
                crate::time::kinakaze_abi_nanosleep(
                    argument1 as *const crate::time::TimeSpec,
                    argument2 as *mut crate::time::TimeSpec,
                )
            };
            to_kernel(res as i64)
        }
        SYS_GETPID => i64::from(crate::process::kinakaze_abi_getpid()),
        SYS_WAIT4 => {
            let res = unsafe {
                crate::exec::kinakaze_abi_wait4(
                    argument1 as c_int,
                    argument2 as *mut c_int,
                    argument3 as c_int,
                    argument4 as *mut c_void,
                )
            };
            to_kernel(res as i64)
        }
        SYS_WAITID => {
            let res = unsafe {
                crate::exec::kinakaze_abi_waitid(
                    argument1 as c_int,
                    argument2 as c_int,
                    argument3 as *mut crate::exec::SigInfo,
                    argument4 as c_int,
                )
            };
            if res == 0 && argument5 != 0 {
                // Linux x86_64 appends a nullable struct rusage to waitid(2).
                unsafe { std::ptr::write_bytes(argument5 as *mut u8, 0, 144) };
            }
            if std::env::var_os("KINAKAZE_POST_WAIT_TRACE").is_some() {
                POST_WAIT_TRACE_ACTIVE.store(true, std::sync::atomic::Ordering::Release);
            }
            kinakaze_vfs::epoll::enable_pipe_state_trace();
            to_kernel(res as i64)
        }
        SYS_SOCKET => {
            let res = unsafe {
                crate::net::kinakaze_abi_socket(
                    argument1 as c_int,
                    argument2 as c_int,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_CONNECT => {
            let res = unsafe {
                crate::net::kinakaze_abi_connect(
                    argument1 as c_int,
                    argument2 as *const u8,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_ACCEPT => {
            let res = unsafe {
                crate::net::kinakaze_abi_accept(
                    argument1 as c_int,
                    argument2 as *mut u8,
                    argument3 as *mut c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SENDTO => {
            let res = unsafe {
                crate::net::kinakaze_abi_sendto(
                    argument1 as c_int,
                    argument2 as *const c_void,
                    argument3 as usize,
                    argument4 as c_int,
                    argument5 as *const u8,
                    argument6 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_RECVFROM => {
            let res = unsafe {
                crate::net::kinakaze_abi_recvfrom(
                    argument1 as c_int,
                    argument2 as *mut c_void,
                    argument3 as usize,
                    argument4 as c_int,
                    argument5 as *mut u8,
                    argument6 as *mut c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SENDMSG => {
            let res = unsafe {
                crate::netdb::kinakaze_abi_sendmsg(
                    argument1 as c_int,
                    argument2 as *const crate::netdb::MsgHdr,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_RECVMSG => {
            let res = unsafe {
                crate::netdb::kinakaze_abi_recvmsg(
                    argument1 as c_int,
                    argument2 as *mut crate::netdb::MsgHdr,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        307 => to_kernel(unsafe {
            crate::misc::kinakaze_abi_sendmmsg(
                argument1 as c_int,
                argument2 as *mut c_void,
                argument3 as u32,
                argument4 as c_int,
            )
        } as i64),
        299 => to_kernel(unsafe {
            crate::misc::kinakaze_abi_recvmmsg(
                argument1 as c_int,
                argument2 as *mut c_void,
                argument3 as u32,
                argument4 as c_int,
                argument5 as *mut c_void,
            )
        } as i64),
        SYS_SHUTDOWN => {
            let res = unsafe {
                crate::net::kinakaze_abi_shutdown(argument1 as c_int, argument2 as c_int)
            };
            to_kernel(res as i64)
        }
        SYS_BIND => {
            let res = unsafe {
                crate::net::kinakaze_abi_bind(
                    argument1 as c_int,
                    argument2 as *const u8,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_LISTEN => {
            let res =
                unsafe { crate::net::kinakaze_abi_listen(argument1 as c_int, argument2 as c_int) };
            to_kernel(res as i64)
        }
        SYS_GETSOCKNAME => {
            let res = unsafe {
                crate::net::kinakaze_abi_getsockname(
                    argument1 as c_int,
                    argument2 as *mut u8,
                    argument3 as *mut c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_GETPEERNAME => {
            let res = unsafe {
                crate::net::kinakaze_abi_getpeername(
                    argument1 as c_int,
                    argument2 as *mut u8,
                    argument3 as *mut c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SOCKETPAIR => {
            let res = unsafe {
                crate::net::kinakaze_abi_socketpair(
                    argument1 as c_int,
                    argument2 as c_int,
                    argument3 as c_int,
                    argument4 as *mut c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SETSOCKOPT => {
            let res = unsafe {
                crate::net::kinakaze_abi_setsockopt(
                    argument1 as c_int,
                    argument2 as c_int,
                    argument3 as c_int,
                    argument4 as *const c_void,
                    argument5 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_GETSOCKOPT => {
            let res = unsafe {
                crate::net::kinakaze_abi_getsockopt(
                    argument1 as c_int,
                    argument2 as c_int,
                    argument3 as c_int,
                    argument4 as *mut c_void,
                    argument5 as *mut c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_EXECVE => {
            let res = unsafe {
                crate::exec::kinakaze_abi_execve(
                    argument1 as *const c_char,
                    argument2 as *const *const c_char,
                    argument3 as *const *const c_char,
                )
            };
            to_kernel(res as i64)
        }
        SYS_EXECVEAT => {
            let res = unsafe {
                crate::exec::kinakaze_abi_execveat(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as *const *const c_char,
                    argument4 as *const *const c_char,
                    argument5 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_EXIT => {
            exit_current_guest_thread(argument1 as i32);
        }
        SYS_EXIT_GROUP => {
            crate::process::kinakaze_abi__exit(argument1 as i32);
        }
        SYS_KILL => {
            let res =
                unsafe { crate::signal::kinakaze_abi_kill(argument1 as c_int, argument2 as c_int) };
            to_kernel(res as i64)
        }
        SYS_UNAME => {
            let res = unsafe {
                crate::userdb::kinakaze_abi_uname(argument1 as *mut crate::userdb::Utsname)
            };
            to_kernel(res as i64)
        }
        SYS_SETHOSTNAME => {
            let res = unsafe {
                crate::userdb::kinakaze_abi_sethostname(
                    argument1 as *const c_char,
                    argument2 as usize,
                )
            };
            to_kernel(res as i64)
        }
        171 => {
            let res = unsafe {
                crate::userdb::kinakaze_abi_setdomainname(
                    argument1 as *const c_char,
                    argument2 as usize,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SEMGET => {
            let res = unsafe {
                crate::sysvipc::kinakaze_abi_semget(
                    argument1 as i32,
                    argument2 as c_int,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SEMOP => {
            let res = unsafe {
                crate::sysvipc::kinakaze_abi_semop(
                    argument1 as c_int,
                    argument2 as *const crate::sysvipc::Sembuf,
                    argument3 as usize,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SEMCTL => {
            let res = unsafe {
                crate::sysvipc::kinakaze_abi_semctl(
                    argument1 as c_int,
                    argument2 as c_int,
                    argument3 as c_int,
                    argument4 as usize,
                )
            };
            to_kernel(res as i64)
        }
        SYS_FCNTL => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_fcntl64(
                    argument1 as c_int,
                    argument2 as c_int,
                    argument3 as usize,
                )
            };
            to_kernel(res as i64)
        }
        SYS_FLOCK => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_flock(argument1 as c_int, argument2 as c_int)
            };
            to_kernel(res as i64)
        }
        149 => to_kernel(
            crate::memory_lock::kinakaze_abi_mlock(argument1 as _, argument2 as usize) as i64,
        ),
        150 => to_kernel(crate::memory_lock::kinakaze_abi_munlock(
            argument1 as _,
            argument2 as usize,
        ) as i64),
        151 => to_kernel(crate::memory_lock::kinakaze_abi_mlockall(argument1 as i32) as i64),
        152 => to_kernel(crate::memory_lock::kinakaze_abi_munlockall() as i64),
        325 => to_kernel(crate::memory_lock::kinakaze_abi_mlock2(
            argument1 as _,
            argument2 as usize,
            argument3 as u32,
        ) as i64),
        SYS_MSYNC => to_kernel(unsafe {
            crate::misc::kinakaze_abi_msync(
                argument1 as *mut c_void,
                argument2 as usize,
                argument3 as c_int,
            )
        } as i64),
        162 => {
            crate::fsextra::kinakaze_abi_sync();
            0
        }
        306 => to_kernel(crate::fsextra::kinakaze_abi_syncfs(argument1 as c_int) as i64),
        SYS_SYNC_FILE_RANGE => to_kernel(crate::fsextra::kinakaze_abi_sync_file_range(
            argument1 as c_int,
            argument2 as i64,
            argument3 as i64,
            argument4 as u32,
        ) as i64),
        SYS_FSYNC => {
            let res = unsafe { crate::fsextra::kinakaze_abi_fsync(argument1 as c_int) };
            to_kernel(res as i64)
        }
        SYS_FDATASYNC => {
            let res = unsafe { crate::fsextra::kinakaze_abi_fdatasync(argument1 as c_int) };
            to_kernel(res as i64)
        }
        SYS_TRUNCATE => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_truncate(argument1 as *const c_char, argument2 as i64)
            };
            to_kernel(res as i64)
        }
        SYS_FTRUNCATE => {
            let res = unsafe { crate::fs::ftruncate(argument1 as c_int, argument2 as i64) };
            to_kernel(res as i64)
        }
        SYS_GETDENTS => {
            let res = unsafe {
                crate::dirent::kinakaze_abi_getdents(
                    argument1 as c_int,
                    argument2 as *mut u8,
                    argument3 as usize,
                )
            };
            to_kernel(res as i64)
        }
        SYS_GETDENTS64 => {
            let res = unsafe {
                crate::dirent::kinakaze_abi_getdents64(
                    argument1 as c_int,
                    argument2 as *mut u8,
                    argument3 as usize,
                )
            };
            to_kernel(res as i64)
        }
        SYS_GETCWD => {
            // The kernel ABI returns the byte count including NUL; libc's
            // `getcwd` wrapper instead returns the buffer pointer and therefore
            // cannot be used for a raw syscall instruction.
            unsafe { crate::fs::raw_getcwd(argument1 as *mut c_char, argument2 as usize) }
        }
        SYS_CHDIR => {
            let res = unsafe { crate::fs::chdir(argument1 as *const c_char) };
            to_kernel(res as i64)
        }
        SYS_FCHDIR => {
            let res = unsafe { crate::fdio::kinakaze_abi_fchdir(argument1 as c_int) };
            to_kernel(res as i64)
        }
        SYS_RENAME => {
            let res = unsafe {
                crate::fs::rename(argument1 as *const c_char, argument2 as *const c_char)
            };
            to_kernel(res as i64)
        }
        SYS_MKDIR => {
            let res = unsafe { crate::fs::mkdir(argument1 as *const c_char, argument2 as u32) };
            to_kernel(res as i64)
        }
        SYS_RMDIR => {
            let res = unsafe { crate::fs::rmdir(argument1 as *const c_char) };
            to_kernel(res as i64)
        }
        SYS_CREAT => {
            let res = unsafe { crate::fs::creat(argument1 as *const c_char, argument2 as u32) };
            to_kernel(res as i64)
        }
        SYS_LINK => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_link(
                    argument1 as *const c_char,
                    argument2 as *const c_char,
                )
            };
            to_kernel(res as i64)
        }
        SYS_UNLINK => {
            let res = unsafe { crate::fs::unlink(argument1 as *const c_char) };
            to_kernel(res as i64)
        }
        SYS_SYMLINK => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_symlink(
                    argument1 as *const c_char,
                    argument2 as *const c_char,
                )
            };
            to_kernel(res as i64)
        }
        SYS_READLINK => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_readlink(
                    argument1 as *const c_char,
                    argument2 as *mut c_char,
                    argument3 as usize,
                )
            };
            to_kernel(res as i64)
        }
        SYS_CHMOD => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_chmod(argument1 as *const c_char, argument2 as u32)
            };
            to_kernel(res as i64)
        }
        SYS_FCHMOD => {
            let res = crate::fsextra::kinakaze_abi_fchmod(argument1 as c_int, argument2 as u32);
            to_kernel(res as i64)
        }
        SYS_CHOWN => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_chown(
                    argument1 as *const c_char,
                    argument2 as u32,
                    argument3 as u32,
                )
            };
            to_kernel(res as i64)
        }
        SYS_FCHOWN => {
            let res = crate::fsextra::kinakaze_abi_fchown(
                argument1 as c_int,
                argument2 as u32,
                argument3 as u32,
            );
            to_kernel(res as i64)
        }
        SYS_LCHOWN => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_lchown(
                    argument1 as *const c_char,
                    argument2 as u32,
                    argument3 as u32,
                )
            };
            to_kernel(res as i64)
        }
        SYS_MKNOD => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_mknod(
                    argument1 as *const c_char,
                    argument2 as u32,
                    argument3,
                )
            };
            to_kernel(res as i64)
        }
        SYS_UMASK => i64::from(crate::fsextra::kinakaze_abi_umask(argument1 as u32)),
        SYS_GETTIMEOFDAY => {
            let res = unsafe {
                crate::time::kinakaze_abi_gettimeofday(
                    argument1 as *mut crate::time::TimeVal,
                    argument2 as *mut c_void,
                )
            };
            to_kernel(res as i64)
        }
        SYS_GETRLIMIT => {
            let res =
                unsafe { kinakaze_abi_getrlimit64(argument1 as c_int, argument2 as *mut RLimit64) };
            to_kernel(res as i64)
        }
        SYS_PRLIMIT64 => match unsafe {
            prlimit64(
                argument1 as u32,
                argument2 as c_int,
                argument3 as *const RLimit64,
                argument4 as *mut RLimit64,
            )
        } {
            Ok(()) => 0,
            Err(error) => -i64::from(error),
        },
        SYS_GETRUSAGE => {
            let res = unsafe {
                crate::userdb::kinakaze_abi_getrusage(
                    argument1 as c_int,
                    argument2 as *mut crate::userdb::RUsage,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SYSINFO => {
            let res = unsafe {
                crate::userdb::kinakaze_abi_sysinfo(argument1 as *mut crate::userdb::SysInfo)
            };
            to_kernel(res as i64)
        }
        SYS_TIMES => {
            let res =
                unsafe { crate::time::kinakaze_abi_times(argument1 as *mut crate::time::Tms) };
            to_kernel(res as i64)
        }
        SYS_SETFSUID => i64::from(crate::userdb::kinakaze_abi_setfsuid(argument1 as u32) as u32),
        SYS_SETFSGID => i64::from(crate::userdb::kinakaze_abi_setfsgid(argument1 as u32) as u32),
        SYS_GETUID => i64::from(crate::userdb::kinakaze_abi_getuid()),
        SYS_GETEUID => i64::from(crate::userdb::kinakaze_abi_geteuid()),
        SYS_GETGID => i64::from(crate::userdb::kinakaze_abi_getgid()),
        SYS_GETEGID => i64::from(crate::userdb::kinakaze_abi_getegid()),
        SYS_SETUID => to_kernel(crate::userdb::kinakaze_abi_setuid(argument1 as u32) as i64),
        SYS_SETGID => to_kernel(crate::userdb::kinakaze_abi_setgid(argument1 as u32) as i64),
        SYS_SETREUID => to_kernel(crate::userdb::kinakaze_abi_setreuid(
            argument1 as u32,
            argument2 as u32,
        ) as i64),
        SYS_SETREGID => to_kernel(crate::userdb::kinakaze_abi_setregid(
            argument1 as u32,
            argument2 as u32,
        ) as i64),
        SYS_GETGROUPS => {
            let res = unsafe {
                crate::userdb::kinakaze_abi_getgroups(argument1 as c_int, argument2 as *mut u32)
            };
            to_kernel(res as i64)
        }
        SYS_SETGROUPS => {
            let res = unsafe {
                crate::userdb::kinakaze_abi_setgroups(argument1 as usize, argument2 as *const u32)
            };
            to_kernel(res as i64)
        }
        SYS_SETRESUID => to_kernel(crate::userdb::kinakaze_abi_setresuid(
            argument1 as u32,
            argument2 as u32,
            argument3 as u32,
        ) as i64),
        SYS_GETRESUID => {
            let res = unsafe {
                crate::userdb::kinakaze_abi_getresuid(
                    argument1 as *mut u32,
                    argument2 as *mut u32,
                    argument3 as *mut u32,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SETRESGID => to_kernel(crate::userdb::kinakaze_abi_setresgid(
            argument1 as u32,
            argument2 as u32,
            argument3 as u32,
        ) as i64),
        SYS_GETRESGID => {
            let res = unsafe {
                crate::userdb::kinakaze_abi_getresgid(
                    argument1 as *mut u32,
                    argument2 as *mut u32,
                    argument3 as *mut u32,
                )
            };
            to_kernel(res as i64)
        }
        SYS_GETPPID => i64::from(crate::process::kinakaze_abi_getppid()),
        SYS_GETPGRP => i64::from(crate::userdb::kinakaze_abi_getpgrp()),
        SYS_SETPGID => to_kernel(crate::userdb::kinakaze_abi_setpgid(
            argument1 as c_int,
            argument2 as c_int,
        ) as i64),
        SYS_SETSID => to_kernel(i64::from(crate::userdb::kinakaze_abi_setsid())),
        SYS_GETPGID => to_kernel(i64::from(crate::userdb::kinakaze_abi_getpgid(
            argument1 as c_int,
        ))),
        SYS_GETSID => to_kernel(i64::from(crate::userdb::kinakaze_abi_getsid(
            argument1 as c_int,
        ))),
        SYS_CAPGET => {
            let res = unsafe {
                crate::userdb::kinakaze_abi_capget(
                    argument1 as *mut c_void,
                    argument2 as *mut c_void,
                )
            };
            to_kernel(res as i64)
        }
        SYS_CAPSET => {
            let res = unsafe {
                crate::userdb::kinakaze_abi_capset(
                    argument1 as *mut c_void,
                    argument2 as *const c_void,
                )
            };
            to_kernel(res as i64)
        }
        SYS_RT_SIGPENDING => {
            // The kernel ABI has an eight-byte signal set. The libc wrapper
            // writes glibc's 128-byte sigset_t and corrupts adjacent guest
            // stack objects when called with a raw syscall buffer.
            if argument2 != size_of::<u64>() as u64 {
                -i64::from(EINVAL)
            } else if argument1 == 0 {
                -i64::from(EFAULT)
            } else {
                unsafe { core::ptr::write(argument1 as *mut u64, kinakaze_vfs::signal::pending()) };
                0
            }
        }
        SYS_RT_SIGTIMEDWAIT => {
            let res = unsafe {
                crate::sigextra::kinakaze_abi_sigtimedwait(
                    argument1 as *const crate::signal::SigSet,
                    argument2 as *mut crate::sigextra::SigInfo,
                    argument3 as *const crate::fdio::TimeSpec,
                )
            };
            to_kernel(res as i64)
        }
        SYS_PERSONALITY => to_kernel(i64::from(kinakaze_abi_personality(argument1))),
        SYS_PRCTL => {
            let result = unsafe {
                kinakaze_abi_prctl(
                    argument1 as c_int,
                    argument2,
                    argument3,
                    argument4,
                    argument5,
                )
            };
            to_kernel(i64::from(result))
        }
        SYS_ARCH_PRCTL => {
            const ARCH_SET_GS: u32 = 0x1001;
            const ARCH_SET_FS: u32 = 0x1002;
            const ARCH_GET_FS: u32 = 0x1003;
            const ARCH_GET_GS: u32 = 0x1004;

            match argument1 as u32 {
                ARCH_SET_FS => {
                    if crate::set_process_thread_pointer(argument2 as usize) {
                        0
                    } else {
                        -22
                    }
                }
                ARCH_GET_FS => mount_api::write_user(
                    argument2 as usize,
                    &crate::process_thread_pointer().to_ne_bytes(),
                )
                .map_or_else(|error| -i64::from(error), |()| 0),
                ARCH_SET_GS => {
                    if kinakaze_tls::set_guest_gs_base(argument2 as usize) {
                        0
                    } else {
                        -1
                    }
                }
                ARCH_GET_GS => mount_api::write_user(
                    argument2 as usize,
                    &kinakaze_tls::guest_gs_base().to_ne_bytes(),
                )
                .map_or_else(|error| -i64::from(error), |()| 0),
                _ => -22,
            }
        }
        SYS_GETTID => i64::from(current_tid()),
        SYS_SETXATTR..=SYS_FREMOVEXATTR => to_kernel(unsafe {
            crate::xattr::syscall_abi_result(
                number, argument1, argument2, argument3, argument4, argument5,
            )
        } as i64),
        SYS_FUTEX => futex_syscall(
            argument1 as *mut c_int,
            argument2 as i32,
            argument3 as u32,
            argument4 as *const KernelTimespec,
            argument5 as *mut c_int,
            argument6 as u32,
        ),
        SYS_SCHED_SETAFFINITY => {
            let outcome = unsafe {
                set_affinity(
                    argument1 as c_int,
                    argument2 as usize,
                    argument3 as *const c_void,
                )
            };
            match outcome {
                Ok(()) => 0,
                Err(error) => -i64::from(error),
            }
        }
        SYS_SCHED_GETAFFINITY => {
            let outcome = unsafe {
                get_affinity(
                    argument1 as c_int,
                    argument2 as usize,
                    argument3 as *mut c_void,
                )
            };
            match outcome {
                Ok(written) => written as i64,
                Err(error) => -i64::from(error),
            }
        }
        SYS_SET_TID_ADDRESS => set_tid_address(argument1 as *mut c_int),
        SYS_CLOCK_SETTIME => {
            let res = unsafe {
                crate::time::kinakaze_abi_clock_settime(
                    argument1 as c_int,
                    argument2 as *const crate::time::TimeSpec,
                )
            };
            to_kernel(res as i64)
        }
        SYS_CLOCK_GETTIME => unsafe {
            clock_gettime(argument1 as i64, argument2 as *mut KernelTimespec)
        },
        SYS_CLOCK_GETRES => {
            let res = unsafe {
                crate::time::kinakaze_abi_clock_getres(
                    argument1 as c_int,
                    argument2 as *mut crate::time::TimeSpec,
                )
            };
            to_kernel(res as i64)
        }
        SYS_CLOCK_NANOSLEEP => {
            let res = unsafe {
                crate::time::kinakaze_abi_clock_nanosleep(
                    argument1 as c_int,
                    argument2 as c_int,
                    argument3 as *const crate::time::TimeSpec,
                    argument4 as *mut crate::time::TimeSpec,
                )
            };
            // POSIX clock_nanosleep returns an errno directly; the raw ABI negates it.
            -i64::from(res)
        }
        SYS_EPOLL_WAIT => {
            let res = unsafe {
                crate::net::kinakaze_abi_epoll_wait(
                    argument1 as c_int,
                    argument2 as *mut kinakaze_vfs::epoll::EpollEvent,
                    argument3 as c_int,
                    argument4 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_EPOLL_CTL => {
            let res = unsafe {
                crate::net::kinakaze_abi_epoll_ctl(
                    argument1 as c_int,
                    argument2 as c_int,
                    argument3 as c_int,
                    argument4 as *const kinakaze_vfs::epoll::EpollEvent,
                )
            };
            to_kernel(res as i64)
        }
        SYS_TGKILL => {
            let tgid = argument1 as c_int;
            let tid = argument2 as c_int;
            let signal = argument3 as c_int;
            if tgid <= 0 || tid <= 0 {
                -i64::from(EINVAL)
            } else if tgid != crate::process::kinakaze_abi_getpid() {
                -i64::from(kinakaze_vfs::job::ESRCH)
            } else {
                let native_tid = crate::fsextra::native_signal_tid(tid);
                match kinakaze_vfs::signal::raise_thread_signal(native_tid, signal) {
                    Ok(()) => {
                        if native_tid == kinakaze_vfs::interrupt::current_thread_id() {
                            kinakaze_vfs::signal::deliver_pending();
                        }
                        0
                    }
                    Err(error) => -i64::from(error),
                }
            }
        }
        SYS_OPENAT => {
            let res = unsafe {
                crate::fs::openat(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as c_int,
                    argument4 as u32,
                )
            };
            to_kernel(res as i64)
        }
        SYS_MKDIRAT => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_mkdirat(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as u32,
                )
            };
            to_kernel(res as i64)
        }
        SYS_NEWFSTATAT => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_fstatat(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as *mut kinakaze_vfs::fs::Stat,
                    argument4 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_UNLINKAT => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_unlinkat(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_RENAMEAT => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_renameat(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as c_int,
                    argument4 as *const c_char,
                )
            };
            to_kernel(res as i64)
        }
        SYS_LINKAT => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_linkat(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as c_int,
                    argument4 as *const c_char,
                    argument5 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SYMLINKAT => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_symlinkat(
                    argument1 as *const c_char,
                    argument2 as c_int,
                    argument3 as *const c_char,
                )
            };
            to_kernel(res as i64)
        }
        SYS_READLINKAT => {
            let result = unsafe {
                crate::fsextra::kinakaze_abi_readlinkat(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as *mut c_char,
                    argument4 as usize,
                )
            };
            to_kernel(result as i64)
        }
        SYS_FCHMODAT | SYS_FCHMODAT2 => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_fchmodat(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as u32,
                    if number == SYS_FCHMODAT2 {
                        argument4 as c_int
                    } else {
                        0
                    },
                )
            };
            to_kernel(res as i64)
        }
        SYS_FCHOWNAT => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_fchownat(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as u32,
                    argument4 as u32,
                    argument5 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_MKNODAT => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_mknodat(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as u32,
                    argument4,
                )
            };
            to_kernel(res as i64)
        }
        SYS_FACCESSAT => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_faccessat(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as c_int,
                    argument4 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_FACCESSAT2 => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_faccessat2(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as c_int,
                    argument4 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_PPOLL => {
            let timeout_ms = if argument3 == 0 {
                -1
            } else {
                let ts = unsafe { &*(argument3 as *const crate::time::TimeSpec) };
                (ts.tv_sec
                    .saturating_mul(1000)
                    .saturating_add(ts.tv_nsec / 1_000_000)) as c_int
            };
            let res = unsafe {
                crate::fdio::kinakaze_abi_poll(
                    argument1 as *mut crate::fdio::PollFd,
                    argument2 as u64,
                    timeout_ms,
                )
            };
            to_kernel(res as i64)
        }
        SYS_UTIMENSAT => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_utimensat(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as *const crate::fdio::TimeSpec,
                    argument4 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_EPOLL_PWAIT | SYS_EPOLL_PWAIT2 => {
            let res = unsafe {
                crate::net::kinakaze_abi_epoll_pwait(
                    argument1 as c_int,
                    argument2 as *mut kinakaze_vfs::epoll::EpollEvent,
                    argument3 as c_int,
                    argument4 as c_int,
                    argument5 as *const c_void,
                )
            };
            to_kernel(res as i64)
        }
        SYS_EVENTFD | SYS_EVENTFD2 => {
            let res =
                unsafe { crate::fdio::kinakaze_abi_eventfd(argument1 as u32, argument2 as c_int) };
            to_kernel(res as i64)
        }
        SYS_ACCEPT4 => {
            let res = unsafe {
                crate::net::kinakaze_abi_accept4(
                    argument1 as c_int,
                    argument2 as *mut u8,
                    argument3 as *mut c_int,
                    argument4 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_EPOLL_CREATE1 => {
            let res = unsafe { crate::net::kinakaze_abi_epoll_create1(argument1 as c_int) };
            to_kernel(res as i64)
        }
        SYS_DUP3 => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_dup3(
                    argument1 as c_int,
                    argument2 as c_int,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_PIPE2 => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_pipe2(argument1 as *mut c_int, argument2 as c_int)
            };
            to_kernel(res as i64)
        }
        SYS_GETRANDOM => unsafe {
            getrandom(
                argument1 as *mut c_void,
                argument2 as usize,
                argument3 as u32,
            )
        },
        SYS_MEMBARRIER => membarrier(argument1 as i64, argument2 as u32),
        SYS_STATX => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_statx(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as c_int,
                    argument4 as u32,
                    argument5 as *mut crate::fsextra::Statx,
                )
            };
            to_kernel(res as i64)
        }
        SYS_CLOSE_RANGE => {
            let first = argument1 as c_int;
            let last = (argument2 as c_int).min(1024);
            let flags = argument3 as u32;
            const CLOSE_RANGE_CLOEXEC: u32 = 1 << 2;
            if (flags & CLOSE_RANGE_CLOEXEC) != 0 {
                for fd in first..=last {
                    let _ = kinakaze_vfs::set_close_on_exec(fd, true);
                }
            } else {
                for fd in first..=last {
                    let _ = kinakaze_vfs::close(fd);
                }
            }
            0
        }
        SYS_REBOOT => {
            if !reboot_magic_is_valid(argument1 as u32, argument2 as u32) {
                return -i64::from(EINVAL);
            }
            if !reboot_command_is_valid(argument3 as c_int) {
                return -i64::from(EINVAL);
            }
            -i64::from(EPERM)
        }
        SYS_SETNS => to_kernel(kinakaze_abi_setns(argument1 as c_int, argument2 as c_int) as i64),
        SYS_UNSHARE => to_kernel(kinakaze_abi_unshare(argument1 as c_int) as i64),
        428 => to_kernel(kinakaze_abi_open_tree(
            argument1 as c_int,
            argument2 as *const c_char,
            argument3 as u32,
        ) as i64),
        429 => to_kernel(kinakaze_abi_move_mount(
            argument1 as c_int,
            argument2 as *const c_char,
            argument3 as c_int,
            argument4 as *const c_char,
            argument5 as u32,
        ) as i64),
        430 => to_kernel(kinakaze_abi_fsopen(argument1 as *const c_char, argument2 as u32) as i64),
        431 => to_kernel(kinakaze_abi_fsconfig(
            argument1 as c_int,
            argument2 as u32,
            argument3 as *const c_char,
            argument4 as *const c_void,
            argument5 as c_int,
        ) as i64),
        432 => {
            to_kernel(
                kinakaze_abi_fsmount(argument1 as c_int, argument2 as u32, argument3 as u32) as i64,
            )
        }
        433 => to_kernel(kinakaze_abi_fspick(
            argument1 as c_int,
            argument2 as *const c_char,
            argument3 as u32,
        ) as i64),
        442 => to_kernel(kinakaze_abi_mount_setattr(
            argument1 as c_int,
            argument2 as *const c_char,
            argument3 as u32,
            argument4 as *const c_void,
            argument5 as usize,
        ) as i64),
        457 => to_kernel(kinakaze_abi_statmount(
            argument1 as *const c_void,
            argument2 as *mut c_void,
            argument3 as usize,
            argument4 as u32,
        ) as i64),
        458 => to_kernel(kinakaze_abi_listmount(
            argument1 as *const c_void,
            argument2 as *mut u64,
            argument3 as usize,
            argument4 as u32,
        ) as i64),
        467 => to_kernel(kinakaze_abi_open_tree_attr(
            argument1 as c_int,
            argument2 as *const c_char,
            argument3 as u32,
            argument4 as *const c_void,
            argument5 as usize,
        ) as i64),
        SYS_MOUNT => {
            let res = unsafe {
                kinakaze_abi_mount(
                    argument1 as *const c_char,
                    argument2 as *const c_char,
                    argument3 as *const c_char,
                    argument4 as u64,
                    argument5 as *const c_void,
                )
            };
            to_kernel(res as i64)
        }
        SYS_UMOUNT2 => {
            let res =
                unsafe { kinakaze_abi_umount2(argument1 as *const c_char, argument2 as c_int) };
            to_kernel(res as i64)
        }
        SYS_PIVOT_ROOT => {
            let res = unsafe {
                kinakaze_abi_pivot_root(argument1 as *const c_char, argument2 as *const c_char)
            };
            to_kernel(res as i64)
        }
        SYS_SECCOMP => {
            let res = unsafe {
                kinakaze_abi_seccomp(argument1 as u32, argument2 as u32, argument3 as *mut c_void)
            };
            to_kernel(res as i64)
        }
        SYS_MEMFD_CREATE => {
            let res =
                unsafe { kinakaze_abi_memfd_create(argument1 as *const c_char, argument2 as u32) };
            to_kernel(res as i64)
        }
        SYS_BPF => unsafe {
            crate::bpf::syscall(argument1, argument2 as *mut c_void, argument3 as usize)
        },
        SYS_OPENAT2 => {
            let mut how = [0u8; 24];
            let size = argument4 as usize;
            let result = (|| {
                if size < 24 {
                    return Err(EINVAL);
                }
                if size > 4096 {
                    return Err(7);
                }
                mount_api::read_user(argument3 as usize, &mut how)?;
                if size > 24 {
                    let mut extra = vec![0; size - 24];
                    mount_api::read_user(argument3 as usize + 24, &mut extra)?;
                    if extra.iter().any(|b| *b != 0) {
                        return Err(7);
                    }
                }
                let word = |i| u64::from_le_bytes(how[i..i + 8].try_into().unwrap());
                let flags = word(0);
                let mode = word(8);
                let resolve = word(16);
                if flags > i32::MAX as u64
                    || mode > 0o7777
                    || (mode != 0 && flags & (64 | 0x400000) == 0)
                {
                    return Err(EINVAL);
                }
                if argument2 == 0 {
                    return Err(EFAULT);
                }
                let path = unsafe { CStr::from_ptr(argument2 as *const c_char) }
                    .to_str()
                    .map_err(|_| EINVAL)?;
                crate::fs::open_with_restart(flags as i32, || {
                    kinakaze_vfs::fs::openat_resolved(
                        argument1 as i32,
                        path,
                        flags as i32,
                        crate::fsextra::creation_mode(mode as u32),
                        resolve,
                    )
                })
            })();
            match result {
                Ok(fd) => fd as i64,
                Err(e) => -i64::from(e),
            }
        }
        SYS_SIGNALFD => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_signalfd(
                    argument1 as c_int,
                    argument2 as *const c_void,
                    argument3 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_SIGNALFD4 => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_signalfd4(
                    argument1 as c_int,
                    argument2 as *const c_void,
                    argument3 as usize,
                    argument4 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_TIMERFD_CREATE => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_timerfd_create(argument1 as c_int, argument2 as c_int)
            };
            to_kernel(res as i64)
        }
        SYS_TIMERFD_SETTIME => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_timerfd_settime(
                    argument1 as c_int,
                    argument2 as c_int,
                    argument3 as *const c_void,
                    argument4 as *mut c_void,
                )
            };
            to_kernel(res as i64)
        }
        SYS_TIMERFD_GETTIME => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_timerfd_gettime(
                    argument1 as c_int,
                    argument2 as *mut c_void,
                )
            };
            to_kernel(res as i64)
        }
        SYS_IO_SETUP | SYS_IO_DESTROY | SYS_IO_GETEVENTS | SYS_IO_SUBMIT | SYS_IO_CANCEL => {
            use kinakaze_vfs::iouring::aio;
            let result = match number {
                SYS_IO_SETUP => aio::setup(argument1 as u32, argument2 as usize),
                SYS_IO_DESTROY => aio::destroy(argument1 as usize),
                SYS_IO_GETEVENTS => aio::getevents(
                    argument1 as usize,
                    argument2 as i64,
                    argument3 as i64,
                    argument4 as usize,
                    argument5 as usize,
                ),
                SYS_IO_SUBMIT => {
                    aio::submit(argument1 as usize, argument2 as i64, argument3 as usize)
                }
                _ => aio::cancel(argument1 as usize, argument2 as usize),
            };
            result.unwrap_or_else(|error| -(error as i64))
        }
        SYS_SPLICE => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_splice(
                    argument1 as c_int,
                    argument2 as *mut i64,
                    argument3 as c_int,
                    argument4 as *mut i64,
                    argument5 as usize,
                    argument6 as u32,
                )
            };
            to_kernel(res as i64)
        }
        SYS_COPY_FILE_RANGE => {
            let res = unsafe {
                crate::fdio::kinakaze_abi_copy_file_range(
                    argument1 as c_int,
                    argument2 as *mut i64,
                    argument3 as c_int,
                    argument4 as *mut i64,
                    argument5 as usize,
                    argument6 as u32,
                )
            };
            to_kernel(res as i64)
        }
        SYS_NAME_TO_HANDLE_AT => {
            let res = unsafe {
                crate::fsextra::kinakaze_abi_name_to_handle_at(
                    argument1 as c_int,
                    argument2 as *const c_char,
                    argument3 as *mut c_void,
                    argument4 as *mut c_int,
                    argument5 as c_int,
                )
            };
            to_kernel(res as i64)
        }
        SYS_OPEN_BY_HANDLE_AT => to_kernel(unsafe {
            crate::fsextra::kinakaze_abi_open_by_handle_at(
                argument1 as c_int,
                argument2 as *const c_void,
                argument3 as c_int,
            )
        } as i64),
        SYS_CHROOT => {
            let res = unsafe { kinakaze_abi_chroot(argument1 as *const c_char) };
            to_kernel(res as i64)
        }
        SYS_SYSLOG => -i64::from(EPERM),
        SYS_FORK => to_kernel(crate::kinakaze_abi_fork() as i64),
        // Raw SYS_clone returns twice at the instruction following `syscall`.
        // Process-shaped forms use the fork coordinator; task-shaped forms use a
        // native thread plus a copied syscall register frame.
        SYS_CLONE => clone_dispatch(
            argument1,
            argument2 as usize,
            argument3 as *mut c_int,
            argument4 as *mut c_int,
            argument5 as usize,
        ),
        435 => unsafe { raw_clone3(argument1 as *const u8, argument2 as usize) },
        SYS_SETRLIMIT => {
            // Wrapper convention: setrlimit takes (resource, limit*).
            let res = unsafe {
                kinakaze_abi_setrlimit64(argument1 as c_int, argument2 as *const RLimit64)
            };
            to_kernel(res as i64)
        }
        SYS_SIGALTSTACK => {
            // The alternate stack is per-thread Linux state shared by the raw
            // syscall and libc wrapper paths.
            let res = unsafe {
                crate::sigextra::kinakaze_abi_sigaltstack(
                    argument1 as *const crate::sigextra::Stack,
                    argument2 as *mut crate::sigextra::Stack,
                )
            };
            to_kernel(res as i64)
        }
        SYS_INOTIFY_INIT => {
            let res = crate::misc::kinakaze_abi_inotify_init1(0);
            to_kernel(res as i64)
        }
        SYS_INOTIFY_INIT1 => {
            let res = crate::misc::kinakaze_abi_inotify_init1(argument1 as c_int);
            to_kernel(res as i64)
        }
        SYS_INOTIFY_ADD_WATCH => {
            let res = unsafe {
                crate::misc::kinakaze_abi_inotify_add_watch(
                    argument1 as c_int,
                    argument2 as *const core::ffi::c_char,
                    argument3 as u32,
                )
            };
            to_kernel(res as i64)
        }
        SYS_INOTIFY_RM_WATCH => {
            let res =
                crate::misc::kinakaze_abi_inotify_rm_watch(argument1 as c_int, argument2 as i32);
            to_kernel(res as i64)
        }
        _ => -i64::from(ENOSYS),
    };
    if !matches!(number, 202 | 228 | 35 | 24 | 281 | 232) {
        if let Some(directory) = std::env::var_os("KINAKAZE_NAMESPACE_TRACE_DIR") {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(
                std::path::PathBuf::from(directory)
                    .join(format!("syscall-errors-{}.log", std::process::id())),
            ) {
                let _ = file.write_all(format!("nr={number} args={argument1:#x},{argument2:#x},{argument3:#x},{argument4:#x},{argument5:#x},{argument6:#x} result={res}\n").as_bytes());
            }
        }
    }
    if number == SYS_CLONE && std::env::var_os("KINAKAZE_CLONE_TRACE").is_some() {
        eprintln!("kinakaze: raw clone dispatcher result={res} ({res:#x})");
    }
    if crate::trace_enabled() {
        let guest_rip = kinakaze_tls::thread_pointer::active_guest_signal_context()
            .map(|(_, rip)| rip)
            .unwrap_or(0);
        eprintln!(
            "kinakaze: [SYSCALL] rip={guest_rip:#x} nr={number} a1={argument1:#x} a2={argument2:#x} a3={argument3:#x} -> res={res}"
        );
    }
    if post_wait_trace {
        eprintln!("kinakaze: [POST-WAIT EXIT] nr={number} res={res}");
    }
    res
}

/// `syscall` — the libc entry point, exported as `syscall`.
///
/// This is *not* the kernel ABI, even though it talks to the same dispatcher.
/// glibc's `syscall()` translates a failed kernel call into `-1` plus `errno`;
/// no Linux program compiled against glibc ever sees `-ENOSYS` from this
/// function. Returning the raw negative value here is a silent contract
/// violation that only surfaces in callers comparing against `-1`:
///
/// ```c
/// ringfd = uv__io_uring_setup(entries, &params);
/// if (ringfd == -1) return;          /* -ENOSYS sails straight past this */
/// if (!(params.features & RSRC_TAGS)) goto fail;
/// fail: uv__close(ringfd);           /* assert(fd > STDERR_FILENO) fires */
/// ```
///
/// That is exactly how Node.js died during `uv_loop_init`: syscall 425
/// (`io_uring_setup`) was unimplemented, `-38` came back instead of `-1`, and
/// libuv fed it to `uv__close` as a descriptor.
///
/// The kernel ABI is preserved in [`kinakaze_abi_syscall_raw`] for the
/// instruction-level trampoline, which patches bare `syscall` instructions
/// issued by runtimes that do their own error decoding.
///
/// # Safety
///
/// Each argument is interpreted according to `number`; the caller must honour
/// the kernel's contract for that call.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_syscall(
    number: i64,
    argument1: u64,
    argument2: u64,
    argument3: u64,
    argument4: u64,
    argument5: u64,
    argument6: u64,
) -> i64 {
    // SAFETY: forwarded unchanged; this wrapper only reinterprets the result.
    let raw = unsafe {
        kinakaze_abi_syscall_raw(
            number, argument1, argument2, argument3, argument4, argument5, argument6,
        )
    };
    // Only the kernel's error band is an error. A successful `mmap` of a high
    // address and any other large positive value can wrap negative in i64, so
    // the range has to be checked, not the sign bit alone.
    if (-4095..0).contains(&raw) {
        set_errno((-raw) as c_int);
        return -1;
    }
    raw
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn syscall(
    number: i64,
    argument1: u64,
    argument2: u64,
    argument3: u64,
    argument4: u64,
    argument5: u64,
    argument6: u64,
) -> i64 {
    unsafe {
        kinakaze_abi_syscall(
            number, argument1, argument2, argument3, argument4, argument5, argument6,
        )
    }
}

/// The raw syscall and exported gettid share the same namespace projection.
fn current_tid() -> c_int {
    crate::fsextra::kinakaze_abi_gettid()
}

const CLONE_VM: u64 = 0x0000_0100;
const CLONE_FS: u64 = 0x0000_0200;
const CLONE_FILES: u64 = 0x0000_0400;
const CLONE_SIGHAND: u64 = 0x0000_0800;
const CLONE_THREAD: u64 = 0x0001_0000;
const CLONE_SYSVSEM: u64 = 0x0004_0000;
const CLONE_SETTLS: u64 = 0x0008_0000;
const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
const CLONE_DETACHED: u64 = 0x0040_0000;
const CLONE_CHILD_SETTID: u64 = 0x0100_0000;

/// A process clone resumes the copied raw-syscall trampoline in both processes.
/// Only the child changes its published guest RSP; the provider's private host
/// stack must remain intact until that trampoline has restored all registers.
fn raw_clone_process(
    flags: u64,
    child_stack: usize,
    parent_tid: *mut c_int,
    child_tid: *mut c_int,
    requested_tls: usize,
) -> i64 {
    let Some((_, instruction)) = kinakaze_tls::thread_pointer::active_guest_signal_context() else {
        return -i64::from(EINVAL);
    };
    let parent = if flags & 0x8000 != 0 {
        let parent = kinakaze_vfs::job::process_info(kinakaze_vfs::job::process_id())
            .map(|info| info.entry.ppid)
            .filter(|&pid| pid != 0);
        if parent.is_none() {
            return -i64::from(EINVAL);
        }
        parent
    } else {
        None
    };
    let result = unsafe {
        crate::clone_process(
            parent,
            initialize_process_namespaces as *const () as usize,
            flags,
        )
    };
    if result > 0 && flags & CLONE_PARENT_SETTID != 0 {
        unsafe {
            parent_tid.write(result);
        }
    }
    if result == 0 {
        if flags & CLONE_SETTLS != 0 && !crate::set_process_thread_pointer(requested_tls) {
            crate::process::terminate_host_process(127);
        }
        if flags & CLONE_CHILD_SETTID != 0 {
            unsafe {
                child_tid.write(crate::process::kinakaze_abi_getpid());
            }
        }
        CHILD_CLEAR_TID.set(if flags & CLONE_CHILD_CLEARTID != 0 {
            child_tid as usize
        } else {
            0
        });
    }
    if result == 0 && child_stack != 0 {
        if !kinakaze_tls::thread_pointer::update_active_guest_signal_context(
            child_stack,
            instruction,
        ) {
            // Fork already created this child. It cannot return on a different
            // stack than requested, nor report a parent-side syscall failure.
            crate::process::terminate_host_process(127);
        }
    }
    i64::from(result)
}

const NEW_NAMESPACE_FLAGS: u64 = 0x7e02_0000;
// Internal marker: clone3 places NEWTIME in bit 7, legacy clone uses that bit for CSIGNAL.
const CLONE3_NEWTIME: u64 = 1 << 63;
pub(crate) unsafe extern "sysv64" fn initialize_process_namespaces(flags: u64) -> i32 {
    let new =
        (flags & NEW_NAMESPACE_FLAGS) as u32 | if flags & CLONE3_NEWTIME != 0 { 0x80 } else { 0 };
    if new != 0 && kinakaze_abi_unshare(new as i32) != 0 {
        return crate::kinakaze_errno();
    }
    if flags & CLONE3_NEWTIME != 0 {
        if let Err(e) = kinakaze_vfs::time_namespace::enter_children() {
            return e;
        }
    }
    if flags & 0x2000_0000 != 0 {
        if let Err(e) = kinakaze_runtime::job::namespaces::enter_new_pid_namespace() {
            return e;
        }
    }
    0
}
fn clone_dispatch(
    flags: u64,
    stack: usize,
    parent_tid: *mut c_int,
    child_tid: *mut c_int,
    tls: usize,
) -> i64 {
    if let Some(directory) = std::env::var_os("KINAKAZE_NAMESPACE_TRACE_DIR") {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(
            std::path::PathBuf::from(directory).join(format!("clone-{}.log", std::process::id())),
        ) {
            let _ = writeln!(
                file,
                "flags={flags:#x} stack={stack:#x} ptid={parent_tid:p} ctid={child_tid:p} tls={tls:#x}"
            );
        }
    }

    if flags & CLONE3_NEWTIME != 0 && flags & CLONE_VM != 0 {
        return -i64::from(EINVAL);
    }
    if flags & CLONE_THREAD != 0 && flags & (0x3000_0000 | 0xff) != 0
        || flags & CLONE_FS != 0 && flags & 0x1002_0000 != 0
        || flags & CLONE_SYSVSEM != 0 && flags & 0x0800_0000 != 0
        || flags & CLONE_SIGHAND != 0 && flags & CLONE_VM == 0
    {
        return -i64::from(EINVAL);
    }
    if (flags & CLONE_PARENT_SETTID != 0 && parent_tid.is_null())
        || (flags & (CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID) != 0 && child_tid.is_null())
    {
        return -i64::from(EFAULT);
    }
    let pid = kinakaze_vfs::job::process_id();
    if flags & CLONE_VM != 0
        && kinakaze_vfs::time_namespace::process_id(pid, false)
            != kinakaze_vfs::time_namespace::process_id(pid, true)
    {
        return -i64::from(EINVAL);
    }
    if flags == (CLONE_VM | 0x4000 | 17) && stack == 0 {
        return to_kernel(unsafe { crate::exec::kinakaze_abi_vfork() } as i64);
    }
    if flags == 17 && stack == 0 {
        return to_kernel(crate::kinakaze_abi_fork() as i64);
    }
    if flags & CLONE_VM != 0 {
        return raw_clone_thread(flags, stack, parent_tid, child_tid, tls);
    }
    let supported = NEW_NAMESPACE_FLAGS
        | CLONE3_NEWTIME
        | 0x8000
        | CLONE_PARENT_SETTID
        | CLONE_CHILD_SETTID
        | CLONE_CHILD_CLEARTID
        | CLONE_SETTLS
        | 0xff;
    if flags & !supported != 0 {
        return -i64::from(ENOSYS);
    }
    if flags & 0xff != 17 {
        return -i64::from(EINVAL);
    }
    raw_clone_process(flags, stack, parent_tid, child_tid, tls)
}
unsafe fn raw_clone3(address: *const u8, size: usize) -> i64 {
    if size < 64 {
        return -i64::from(EINVAL);
    }
    if size > 4096 {
        return -7;
    }
    if address.is_null() {
        return -i64::from(EFAULT);
    }
    let bytes = unsafe { std::slice::from_raw_parts(address, size) };
    if bytes
        .get(88..)
        .is_some_and(|tail| tail.iter().any(|v| *v != 0))
    {
        return -7;
    }
    let word = |index: usize| {
        bytes
            .get(index * 8..index * 8 + 8)
            .map(|v| u64::from_ne_bytes(v.try_into().unwrap()))
            .unwrap_or(0)
    };
    let flags = word(0);
    let signal = word(4);
    let stack = word(5);
    let length = word(6);
    if signal > 64 || flags & (0x7f | CLONE3_NEWTIME) != 0 || (stack == 0) != (length == 0) {
        return -i64::from(EINVAL);
    }
    // set_tid and cgroup placement require their own registry transactions.
    if word(9) != 0 || flags & (0x1000 | 0x2000_0000_0) != 0 {
        return -i64::from(ENOSYS);
    }
    let Some(top) = stack.checked_add(length) else {
        return -i64::from(EINVAL);
    };
    let flags = (flags & !0x80) | if flags & 0x80 != 0 { CLONE3_NEWTIME } else { 0 };
    clone_dispatch(
        flags | signal,
        top as usize,
        word(3) as *mut c_int,
        word(2) as *mut c_int,
        word(7) as usize,
    )
}

/// Provider-side initialization that must happen on the newly created thread.
/// The executable owns register restoration, but libc owns Linux signal-mask and
/// clear-child-tid state, so neither module guesses the other's TLS layout.
unsafe extern "sysv64" fn initialize_raw_clone_thread(
    flags: u64,
    child_tid: *mut c_int,
    thread_pointer: usize,
    signal_mask: u64,
    inheritance: usize,
) -> i32 {
    // The parent retains the packet until this initializer acknowledges it.
    unsafe {
        (&*(inheritance as *const kinakaze_vfs::fs_context::Inheritance)).adopt();
    }
    if !crate::set_process_thread_pointer(thread_pointer) {
        return EINVAL;
    }
    let _ = kinakaze_vfs::signal::swap_blocked_mask(signal_mask);
    let tid = current_tid();
    if flags & CLONE_CHILD_SETTID != 0 {
        // SAFETY: raw_clone_thread validated the flag/pointer pair; the address
        // remains process-shared under mandatory CLONE_VM.
        unsafe { child_tid.write(tid) };
    }
    CHILD_CLEAR_TID.set(if flags & CLONE_CHILD_CLEARTID != 0 {
        child_tid as usize
    } else {
        0
    });
    0
}

/// Implements the resource-sharing clone shape used for Linux tasks/threads.
/// Unsupported sharing combinations fail instead of inheriting this process's
/// necessarily shared cwd or descriptor table under a different promise.
fn raw_clone_thread(
    flags: u64,
    child_stack: usize,
    parent_tid: *mut c_int,
    child_tid: *mut c_int,
    requested_tls: usize,
) -> i64 {
    const EXIT_SIGNAL_MASK: u64 = 0xff;
    const REQUIRED: u64 = CLONE_VM | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;
    const SUPPORTED: u64 = REQUIRED
        | CLONE_FS
        | CLONE_SYSVSEM
        | CLONE_SETTLS
        | CLONE_PARENT_SETTID
        | CLONE_CHILD_CLEARTID
        | CLONE_DETACHED
        | CLONE_CHILD_SETTID;

    if flags & EXIT_SIGNAL_MASK != 0 {
        return -i64::from(EINVAL);
    }
    if flags & REQUIRED != REQUIRED || flags & !SUPPORTED != 0 {
        return -i64::from(ENOSYS);
    }
    if child_stack == 0
        || (flags & CLONE_PARENT_SETTID != 0 && parent_tid.is_null())
        || (flags & (CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID) != 0 && child_tid.is_null())
    {
        return -i64::from(EFAULT);
    }
    let thread_pointer = if flags & CLONE_SETTLS != 0 {
        requested_tls
    } else {
        crate::process_thread_pointer()
    };
    if thread_pointer == 0 {
        return -i64::from(EINVAL);
    }
    let entry = crate::process_clone_thread_entry();
    if entry == 0 {
        return -i64::from(ENOSYS);
    }
    type CloneThread = unsafe extern "sysv64" fn(
        usize,
        usize,
        usize,
        u64,
        *mut c_int,
        *mut c_int,
        u64,
        usize,
    ) -> i64;
    let inheritance = match kinakaze_vfs::fs_context::capture(flags & CLONE_FS != 0) {
        Ok(value) => value,
        Err(e) => return -i64::from(e),
    };
    // Reserve the process count before the native thread can run and exit.
    let _ = kinakaze_runtime::process_thread_started();
    let clone: CloneThread = unsafe { core::mem::transmute(entry) };
    let trace = std::env::var_os("KINAKAZE_CLONE_TRACE").is_some();
    if trace {
        eprintln!(
            "kinakaze: clone provider enter entry={entry:#x} stack={child_stack:#x} tp={thread_pointer:#x} flags={flags:#x} child_tid={child_tid:p}"
        );
    }
    let tid = unsafe {
        clone(
            child_stack,
            thread_pointer,
            initialize_raw_clone_thread as *const () as usize,
            flags,
            parent_tid,
            child_tid,
            kinakaze_vfs::signal::blocked_mask(),
            &inheritance as *const _ as usize,
        )
    };
    if trace {
        eprintln!("kinakaze: clone provider returned tid={tid} ({tid:#x})");
    }
    if tid < 0 {
        let _ = kinakaze_runtime::process_thread_finished();
        return tid;
    }
    tid
}

fn clear_child_tid() {
    let address = CHILD_CLEAR_TID.replace(0);
    if address == 0 {
        return;
    }
    let value = address as *mut AtomicI32;
    // SAFETY: CLONE_CHILD_CLEARTID requires an aligned writable int in the shared
    // address space. Release ordering publishes all preceding thread writes.
    unsafe { (*value).store(0, Ordering::Release) };
    // Linux performs FUTEX_WAKE(1), and the exact waiter count comes from the
    // same process-private futex queue used by raw SYS_futex.
    if let Ok(address) = FutexAddress::resolve(value.cast(), false) {
        let _ = futex_wake(address, 1, FUTEX_BITSET_MATCH_ANY);
    }
}

/// Raw `SYS_exit` terminates only the calling Linux task. The last remaining
/// task performs process exit so the job registry receives the final status.
fn exit_current_guest_thread(status: i32) -> ! {
    clear_child_tid();
    if kinakaze_runtime::retire_guest_thread() {
        crate::process::kinakaze_abi__exit(status);
    }
    // The process remains alive, so only this native thread is retired.
    unsafe { ExitThread(status as u32) }
}

/// Serves `SYS_set_tid_address`, returning the caller's tid.
///
/// The address is retained as per-thread kernel state. Raw `SYS_exit` atomically
/// clears it and wakes address waiters, which is the Linux join contract used by
/// both libc and language runtimes.
fn set_tid_address(address: *mut c_int) -> i64 {
    CHILD_CLEAR_TID.set(address as usize);
    i64::from(current_tid())
}

#[cfg(test)]
mod mount_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_process_group_syscalls_report_linux_errno() {
        unsafe {
            assert_eq!(
                kinakaze_abi_syscall_raw(SYS_SETPGID, u64::MAX, 0, 0, 0, 0, 0),
                -22
            );
            assert_eq!(
                kinakaze_abi_syscall_raw(SYS_GETPGID, u64::MAX, 0, 0, 0, 0, 0),
                -3
            );
            assert_eq!(
                kinakaze_abi_syscall_raw(SYS_GETSID, u64::MAX, 0, 0, 0, 0, 0),
                -3
            );
        }
    }

    /// Clears errno so a later assertion cannot read a stale value.
    fn clear_errno() {
        crate::set_errno(0);
    }

    #[test]
    fn sched_yield_always_reports_success() {
        // Linux returns 0 unconditionally. `SwitchToThread` returning "nobody was
        // waiting" is not a failure, and this pins that it is not propagated: on an
        // idle machine several of these calls will find no other runnable thread.
        for _ in 0..64 {
            assert_eq!(kinakaze_abi_sched_yield(), 0);
        }
    }

    #[test]
    fn affinity_reflects_the_real_processor_set() {
        // A full Linux cpu_set_t, so the ceiling documentation is exercised against
        // the size a real caller passes.
        let mut set = [0u8; 128];
        // SAFETY: `set` is 128 writable bytes.
        let result = unsafe {
            kinakaze_abi_sched_getaffinity(0, set.len(), set.as_mut_ptr().cast::<c_void>())
        };
        assert_eq!(result, 0, "the wrapper form reports 0 on success");

        let mut mask = 0usize;
        // SAFETY: the first word of a 128-byte buffer is readable.
        unsafe {
            core::ptr::copy_nonoverlapping(
                set.as_ptr(),
                (&raw mut mask).cast::<u8>(),
                size_of::<usize>(),
            );
        }
        assert_ne!(mask, 0, "this thread must be able to run somewhere");

        // The mask has to agree with the processor count the host reports, not just
        // be non-zero: a hardcoded 1 would pass the check above.
        let online = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        assert_eq!(
            mask.count_ones() as usize,
            online.min(64),
            "the set bits must be the processors actually available"
        );

        // Bits past the first word are cleared, never left as whatever was on the
        // stack; a caller asking about CPU 200 must see it clear.
        assert!(
            set[size_of::<usize>()..].iter().all(|byte| *byte == 0),
            "the tail of the cpu_set_t must be zeroed"
        );

        // A buffer too small to hold one word cannot express even CPU 0, and a
        // length that is not a whole number of words describes a trailing partial
        // word the kernel refuses to interpret. Both are EINVAL on Linux.
        for bad in [4usize, 12, 20] {
            clear_errno();
            let mut tiny = [0u8; 32];
            // SAFETY: `tiny` is 32 writable bytes, more than any `bad` length.
            let result = unsafe {
                kinakaze_abi_sched_getaffinity(0, bad, tiny.as_mut_ptr().cast::<c_void>())
            };
            assert_eq!(result, -1, "size {bad} must be rejected");
            assert_eq!(kinakaze_tls::errno(), EINVAL);
        }
        // The word multiples around them are accepted.
        for good in [8usize, 16, 128] {
            let mut set = [0u8; 128];
            // SAFETY: `set` is 128 writable bytes, at least `good`.
            let result = unsafe {
                kinakaze_abi_sched_getaffinity(0, good, set.as_mut_ptr().cast::<c_void>())
            };
            assert_eq!(result, 0, "size {good} must be accepted");
        }

        // Another thread cannot be described from here, and is refused rather than
        // answered with this thread's mask.
        clear_errno();
        // SAFETY: `set` is writable; the call fails before touching it.
        let elsewhere = 0x7FFF_FFFE;
        let result = unsafe {
            kinakaze_abi_sched_getaffinity(elsewhere, set.len(), set.as_mut_ptr().cast::<c_void>())
        };
        assert_eq!(result, -1);
        assert_eq!(kinakaze_tls::errno(), ESRCH);
    }

    #[test]
    fn affinity_round_trips_through_set_and_get() {
        let mut original = [0u8; 128];
        // SAFETY: 128 writable bytes.
        assert_eq!(
            unsafe {
                kinakaze_abi_sched_getaffinity(
                    0,
                    original.len(),
                    original.as_mut_ptr().cast::<c_void>(),
                )
            },
            0
        );
        let mut system = 0usize;
        // SAFETY: the first word is readable.
        unsafe {
            core::ptr::copy_nonoverlapping(
                original.as_ptr(),
                (&raw mut system).cast::<u8>(),
                size_of::<usize>(),
            );
        }

        // Pin to the lowest processor actually available, which is a subset of the
        // system mask and therefore a legal request.
        let single = 1usize << system.trailing_zeros();
        let mut request = [0u8; 128];
        // SAFETY: the first word of a 128-byte buffer is writable.
        unsafe {
            core::ptr::copy_nonoverlapping(
                (&raw const single).cast::<u8>(),
                request.as_mut_ptr(),
                size_of::<usize>(),
            );
        }
        // SAFETY: `request` is 128 readable bytes.
        let result = unsafe {
            kinakaze_abi_sched_setaffinity(0, request.len(), request.as_ptr().cast::<c_void>())
        };
        assert_eq!(result, 0, "pinning to an available processor must succeed");

        let mut read_back = [0u8; 128];
        // SAFETY: 128 writable bytes.
        assert_eq!(
            unsafe {
                kinakaze_abi_sched_getaffinity(
                    0,
                    read_back.len(),
                    read_back.as_mut_ptr().cast::<c_void>(),
                )
            },
            0
        );
        let mut observed = 0usize;
        // SAFETY: the first word is readable.
        unsafe {
            core::ptr::copy_nonoverlapping(
                read_back.as_ptr(),
                (&raw mut observed).cast::<u8>(),
                size_of::<usize>(),
            );
        }
        assert_eq!(
            observed, single,
            "the mask read back must be the one that was set"
        );

        // An empty set is EINVAL: a thread has to be able to run somewhere.
        clear_errno();
        let empty = [0u8; 128];
        // SAFETY: 128 readable bytes.
        let result = unsafe {
            kinakaze_abi_sched_setaffinity(0, empty.len(), empty.as_ptr().cast::<c_void>())
        };
        assert_eq!(result, -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);

        // A mask containing only unavailable CPUs has an empty intersection.
        clear_errno();
        let mut high = [0u8; 128];
        high[64] = 1;
        // SAFETY: 128 readable bytes.
        let result = unsafe {
            kinakaze_abi_sched_setaffinity(0, high.len(), high.as_ptr().cast::<c_void>())
        };
        assert_eq!(result, -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);

        let all = [0xffu8; 128];
        assert_eq!(
            unsafe { kinakaze_abi_sched_setaffinity(0, all.len(), all.as_ptr().cast()) },
            0
        );
        let mut reset = [0u8; 128];
        assert_eq!(
            unsafe { kinakaze_abi_sched_getaffinity(0, reset.len(), reset.as_mut_ptr().cast()) },
            0
        );
        assert_eq!(
            usize::from_ne_bytes(reset[..size_of::<usize>()].try_into().unwrap()),
            affinity_masks().unwrap().0
        );

        // Restore the original mask so the rest of the suite is not left pinned.
        // SAFETY: 128 readable bytes holding the mask captured at entry.
        assert_eq!(
            unsafe {
                kinakaze_abi_sched_setaffinity(
                    0,
                    original.len(),
                    original.as_ptr().cast::<c_void>(),
                )
            },
            0
        );
    }

    #[test]
    fn rlimit64_reports_figures_this_project_actually_controls() {
        assert_eq!(size_of::<RLimit64>(), 16, "two 64-bit words, no padding");

        let read = |resource: c_int| {
            let mut limit = RLimit64::default();
            // SAFETY: `limit` is a writable local.
            let result = unsafe { kinakaze_abi_getrlimit64(resource, &raw mut limit) };
            assert_eq!(result, 0, "resource {resource} must be readable");
            limit
        };

        // Report process policy, independently of the sparse table's capacity.
        let nofile = read(RLIMIT_NOFILE);
        assert_eq!(
            (nofile.rlim_cur, nofile.rlim_max),
            kinakaze_vfs::job::nofile_limits(0).unwrap()
        );
        assert!(
            nofile.rlim_cur <= nofile.rlim_max && nofile.rlim_max <= kinakaze_vfs::MAX_FDS as u64
        );

        // The loader's guest stack block, not the Windows thread stack.
        let stack = read(RLIMIT_STACK);
        assert_eq!(stack.rlim_cur, GUEST_STACK_SIZE);
        assert_eq!(stack.rlim_max, RLIM64_INFINITY);

        // Genuinely zero: no Linux core file is ever written here.
        let core = read(RLIMIT_CORE);
        assert_eq!((core.rlim_cur, core.rlim_max), (0, 0));

        // An architectural address-space ceiling is not a per-process quota.
        // These resources have no configured enforcement and report unlimited.
        for resource in [
            RLIMIT_CPU,
            RLIMIT_FSIZE,
            RLIMIT_NPROC,
            RLIMIT_AS,
            RLIMIT_DATA,
        ] {
            let limit = read(resource);
            assert_eq!(limit.rlim_cur, RLIM64_INFINITY);
            assert_eq!(limit.rlim_max, RLIM64_INFINITY);
        }

        // Out-of-range and null are rejected.
        clear_errno();
        let mut limit = RLimit64::default();
        // SAFETY: `limit` is a writable local.
        assert_eq!(unsafe { kinakaze_abi_getrlimit64(99, &raw mut limit) }, -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        clear_errno();
        // SAFETY: passing null is the case under test.
        assert_eq!(
            unsafe { kinakaze_abi_getrlimit64(RLIMIT_NOFILE, core::ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EFAULT);
    }

    #[test]
    fn setrlimit64_only_succeeds_for_retained_and_enforced_state() {
        // A state-changing request for an unsupported resource is explicit.
        let mut current = RLimit64::default();
        // SAFETY: `current` is a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_getrlimit64(RLIMIT_STACK, &raw mut current) },
            0
        );
        let raise = RLimit64 {
            rlim_cur: current.rlim_max,
            rlim_max: current.rlim_max,
        };
        clear_errno();
        // SAFETY: `raise` is a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_setrlimit64(RLIMIT_STACK, &raw const raise) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EOPNOTSUPP);

        // Asking for exactly what is in force succeeds.
        // SAFETY: `current` is a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_setrlimit64(RLIMIT_STACK, &raw const current) },
            0
        );

        // RLIMIT_NOFILE is both retained and enforced, so lowering it succeeds
        // and is observable through getrlimit64.
        let mut nofile = RLimit64::default();
        // SAFETY: `nofile` is a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_getrlimit64(RLIMIT_NOFILE, &raw mut nofile) },
            0
        );
        let smaller = RLimit64 {
            rlim_cur: nofile.rlim_cur - 1,
            rlim_max: nofile.rlim_max - 1,
        };
        // SAFETY: `smaller` is a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_setrlimit64(RLIMIT_NOFILE, &raw const smaller) },
            0
        );
        let mut observed = RLimit64::default();
        // SAFETY: `observed` is a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_getrlimit64(RLIMIT_NOFILE, &raw mut observed) },
            0
        );
        assert_eq!(observed, smaller);
        // Restore the process-wide limit before leaving the test.
        // SAFETY: `nofile` is a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_setrlimit64(RLIMIT_NOFILE, &raw const nofile) },
            0
        );

        // Soft above hard is malformed, and Linux checks it before anything else.
        clear_errno();
        let inverted = RLimit64 {
            rlim_cur: 8192,
            rlim_max: 1024,
        };
        // SAFETY: `inverted` is a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_setrlimit64(RLIMIT_NOFILE, &raw const inverted) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn memory_rlimits_reject_changes_without_enforcement() {
        let read = |resource| {
            let mut value = RLimit64::default();
            assert_eq!(
                unsafe { kinakaze_abi_getrlimit64(resource, &raw mut value) },
                0
            );
            value
        };
        for resource in [RLIMIT_AS, RLIMIT_DATA] {
            let original = read(resource);
            assert_eq!(original.rlim_cur, RLIM64_INFINITY);
            assert_eq!(original.rlim_max, RLIM64_INFINITY);
            // An identical limit is a true no-op through either entry point.
            assert_eq!(
                unsafe { kinakaze_abi_setrlimit64(resource, &raw const original) },
                0
            );
            assert_eq!(
                unsafe {
                    kinakaze_abi_syscall_raw(
                        SYS_SETRLIMIT,
                        resource as u64,
                        &raw const original as u64,
                        0,
                        0,
                        0,
                        0,
                    )
                },
                0
            );
            for requested in [
                RLimit64 {
                    rlim_cur: 1 << 30,
                    rlim_max: original.rlim_max,
                },
                RLimit64 {
                    rlim_cur: 1 << 30,
                    rlim_max: 1 << 30,
                },
            ] {
                assert_ne!(requested, original);
                assert_eq!(
                    unsafe { kinakaze_abi_setrlimit64(resource, &raw const requested) },
                    -1
                );
                assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EOPNOTSUPP);
                assert_eq!(read(resource), original);
                assert_eq!(
                    unsafe {
                        kinakaze_abi_syscall_raw(
                            SYS_SETRLIMIT,
                            resource as u64,
                            &raw const requested as u64,
                            0,
                            0,
                            0,
                            0,
                        )
                    },
                    -i64::from(kinakaze_vfs::EOPNOTSUPP)
                );
                assert_eq!(read(resource), original);
                assert_eq!(
                    unsafe {
                        kinakaze_abi_syscall_raw(
                            SYS_PRLIMIT64,
                            0,
                            resource as u64,
                            &raw const requested as u64,
                            0,
                            0,
                            0,
                        )
                    },
                    -i64::from(kinakaze_vfs::EOPNOTSUPP)
                );
                assert_eq!(read(resource), original);
            }
        }
    }

    #[test]
    fn prctl_thread_name_round_trips() {
        let name = c"kinakaze-test";
        // SAFETY: a null-terminated literal, and the remaining arguments are unused
        // by PR_SET_NAME.
        let result = unsafe { kinakaze_abi_prctl(PR_SET_NAME, name.as_ptr() as u64, 0, 0, 0) };
        assert_eq!(result, 0, "PR_SET_NAME must succeed");

        let mut buffer = [0xAAu8; TASK_COMM_LEN];
        // SAFETY: `buffer` is 16 writable bytes, which is what PR_GET_NAME requires.
        let result =
            unsafe { kinakaze_abi_prctl(PR_GET_NAME, buffer.as_mut_ptr() as u64, 0, 0, 0) };
        assert_eq!(result, 0, "PR_GET_NAME must succeed");
        let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(0);
        assert_eq!(
            &buffer[..end],
            name.to_bytes(),
            "the name read back must be the one that was set"
        );

        // The kernel truncates at 16 bytes including the terminator rather than
        // failing, so 15 bytes of a longer name survive.
        let long = c"0123456789abcdefghij";
        // SAFETY: a null-terminated literal.
        assert_eq!(
            unsafe { kinakaze_abi_prctl(PR_SET_NAME, long.as_ptr() as u64, 0, 0, 0) },
            0
        );
        let mut buffer = [0xAAu8; TASK_COMM_LEN];
        // SAFETY: 16 writable bytes.
        assert_eq!(
            unsafe { kinakaze_abi_prctl(PR_GET_NAME, buffer.as_mut_ptr() as u64, 0, 0, 0) },
            0
        );
        assert_eq!(&buffer[..15], b"0123456789abcde");
        assert_eq!(buffer[15], 0, "the field must stay null-terminated");

        // A null pointer is EFAULT rather than a crash.
        clear_errno();
        // SAFETY: passing null is the case under test.
        assert_eq!(unsafe { kinakaze_abi_prctl(PR_SET_NAME, 0, 0, 0, 0) }, -1);
        assert_eq!(kinakaze_tls::errno(), EFAULT);
    }

    #[test]
    fn prctl_dumpable_is_tracked_and_validated() {
        // SAFETY: PR_GET_DUMPABLE reads no pointer.
        let original = unsafe { kinakaze_abi_prctl(PR_GET_DUMPABLE, 0, 0, 0, 0) };
        assert!(
            (0..=2).contains(&original),
            "dumpable must be one of the three Linux modes"
        );

        for mode in [0, 1, 2] {
            // SAFETY: PR_SET_DUMPABLE reads no pointer.
            assert_eq!(
                unsafe { kinakaze_abi_prctl(PR_SET_DUMPABLE, mode, 0, 0, 0) },
                0
            );
            // SAFETY: as above.
            assert_eq!(
                unsafe { kinakaze_abi_prctl(PR_GET_DUMPABLE, 0, 0, 0, 0) },
                mode as c_int,
                "the mode read back must be the one that was set"
            );
        }

        // Linux rejects anything outside 0..=2.
        clear_errno();
        // SAFETY: no pointer is read.
        assert_eq!(
            unsafe { kinakaze_abi_prctl(PR_SET_DUMPABLE, 7, 0, 0, 0) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);

        // SAFETY: no pointer is read.
        unsafe { kinakaze_abi_prctl(PR_SET_DUMPABLE, original as u64, 0, 0, 0) };
    }

    #[test]
    fn prctl_parent_death_signal_round_trips_and_validates() {
        let mut original = -1;
        assert_eq!(
            unsafe { kinakaze_abi_prctl(PR_GET_PDEATHSIG, (&raw mut original) as u64, 0, 0, 0,) },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_prctl(PR_SET_PDEATHSIG, 9, 0, 0, 0) },
            0
        );
        let mut signal = -1;
        assert_eq!(
            unsafe { kinakaze_abi_prctl(PR_GET_PDEATHSIG, (&raw mut signal) as u64, 0, 0, 0,) },
            0
        );
        assert_eq!(signal, 9);

        clear_errno();
        assert_eq!(
            unsafe { kinakaze_abi_prctl(PR_SET_PDEATHSIG, 65, 0, 0, 0) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        clear_errno();
        assert_eq!(
            unsafe { kinakaze_abi_prctl(PR_GET_PDEATHSIG, 0, 0, 0, 0) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EFAULT);

        assert_eq!(
            unsafe { kinakaze_abi_prctl(PR_SET_PDEATHSIG, original as u64, 0, 0, 0) },
            0
        );
    }

    #[test]
    fn prctl_rejects_unknown_options() {
        // An unrecognised option is EINVAL, as on Linux.
        clear_errno();
        // SAFETY: no pointer is read.
        assert_eq!(unsafe { kinakaze_abi_prctl(9999, 0, 0, 0, 0) }, -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn syscall_raw_uses_the_raw_negative_return_convention() {
        // The kernel ABI. A patched `syscall` instruction is answered in the
        // kernel's own convention: a negative errno, never -1. Runtimes that
        // emit raw `syscall` instructions decode this themselves, so handing
        // them -1 would lose the errno entirely.
        // SAFETY: an unrecognised number dereferences nothing.
        let result = unsafe { kinakaze_abi_syscall_raw(999_999, 0, 0, 0, 0, 0, 0) };
        assert_eq!(
            result,
            -i64::from(ENOSYS),
            "an unknown syscall must return -ENOSYS as a negative value"
        );
        assert_ne!(
            result, -1,
            "-1 would be the libc convention, not the kernel's"
        );

        // The refusals report the same errno on the raw path as through their
        // wrappers, negated.
        for number in [SYS_SWAPON, SYS_SWAPOFF] {
            // SAFETY: each of these ignores its arguments.
            let result = unsafe { kinakaze_abi_syscall_raw(number, 0, 0, 0, 0, 0, 0) };
            assert_eq!(result, -i64::from(ENOSYS), "syscall {number}");
        }
        for number in [SYS_SYSLOG] {
            // SAFETY: each of these ignores its arguments.
            let result = unsafe { kinakaze_abi_syscall_raw(number, 0, 0, 0, 0, 0, 0) };
            assert_eq!(result, -i64::from(EPERM), "syscall {number}");
        }

        // futex null uaddr is EFAULT
        // SAFETY: the number is dispatched with null uaddr.
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(SYS_FUTEX, 0, 0, 0, 0, 0, 0) },
            -i64::from(EFAULT)
        );
    }

    #[test]
    fn raw_statfs_reports_the_synthetic_cgroup_filesystem() {
        let path = c"/sys/fs/cgroup";
        let mut value = crate::fsextra::Statfs::default();
        let result = unsafe {
            kinakaze_abi_syscall_raw(
                SYS_STATFS,
                path.as_ptr() as u64,
                (&raw mut value) as u64,
                0,
                0,
                0,
                0,
            )
        };
        assert_eq!(result, 0);
        assert_eq!(value.f_type, crate::fsextra::CGROUP2_SUPER_MAGIC);

        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(SYS_STATFS, path.as_ptr() as u64, 0, 0, 0, 0, 0) },
            -i64::from(EFAULT)
        );
    }

    #[test]
    fn raw_getcwd_returns_kernel_byte_count() {
        let expected = kinakaze_vfs::fs::getcwd();
        let mut buffer = [0u8; 4096];
        let result = unsafe {
            kinakaze_abi_syscall_raw(
                SYS_GETCWD,
                buffer.as_mut_ptr() as u64,
                buffer.len() as u64,
                0,
                0,
                0,
                0,
            )
        };
        assert_eq!(result, (expected.len() + 1) as i64);
        assert_eq!(&buffer[..expected.len()], expected.as_bytes());
        assert_eq!(buffer[expected.len()], 0);

        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(SYS_GETCWD, 0, buffer.len() as u64, 0, 0, 0, 0) },
            -i64::from(EFAULT)
        );
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_GETCWD,
                    buffer.as_mut_ptr() as u64,
                    expected.len() as u64,
                    0,
                    0,
                    0,
                    0,
                )
            },
            -i64::from(kinakaze_vfs::ERANGE)
        );
    }

    #[test]
    fn raw_ownership_syscalls_reach_the_common_filesystem_implementation() {
        let native =
            std::env::temp_dir().join(format!("kinakaze-raw-chown-{}", std::process::id()));
        std::fs::write(&native, b"ownership").unwrap();
        let path = std::ffi::CString::new(kinakaze_vfs::to_guest_path(&native)).unwrap();
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(SYS_CHOWN, path.as_ptr() as u64, 0, 0, 0, 0, 0,) },
            0
        );
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_FCHOWNAT,
                    (-100i64) as u64,
                    path.as_ptr() as u64,
                    0,
                    0,
                    0,
                    0,
                )
            },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(SYS_CHOWN, 0, 0, 0, 0, 0, 0) },
            -i64::from(EFAULT)
        );
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(SYS_FCHOWN, u64::MAX, 0, 0, 0, 0, 0) },
            -i64::from(kinakaze_vfs::EBADF)
        );
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(SYS_MKNOD, 0, 0, 0, 0, 0, 0) },
            -i64::from(EFAULT)
        );
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(SYS_MKNODAT, (-100i64) as u64, 0, 0, 0, 0, 0) },
            -i64::from(EFAULT)
        );
        std::fs::remove_file(native).unwrap();
    }

    #[test]
    fn syscall_futex_relative_absolute_wake_and_requeue() {
        use crate::time::TimeSpec;
        use kinakaze_vfs::ETIMEDOUT;
        const FUTEX_BITSET_MATCH_ANY: u32 = 0xffff_ffff;

        let mut uaddr1 = 0u32;
        let mut uaddr2 = 0u32;

        // 1. FUTEX_WAIT with relative 50ms timeout
        let rel_ts = TimeSpec {
            tv_sec: 0,
            tv_nsec: 50_000_000,
        };
        let start = std::time::Instant::now();
        let res = unsafe {
            kinakaze_abi_syscall_raw(
                SYS_FUTEX,
                (&raw mut uaddr1) as u64,
                (FUTEX_WAIT | FUTEX_PRIVATE_FLAG) as u64,
                0,
                (&raw const rel_ts) as u64,
                0,
                0,
            )
        };
        let elapsed = start.elapsed();
        assert_eq!(
            res,
            -i64::from(ETIMEDOUT),
            "FUTEX_WAIT with relative timeout should return ETIMEDOUT"
        );
        assert!(
            elapsed >= std::time::Duration::from_millis(30)
                && elapsed < std::time::Duration::from_secs(2),
            "FUTEX_WAIT relative timeout took {:?}",
            elapsed
        );

        // 2. FUTEX_WAIT_BITSET with absolute CLOCK_REALTIME 50ms timeout (Epoch ~1.75B seconds)
        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let abs_target = now_epoch + std::time::Duration::from_millis(50);
        let abs_ts = TimeSpec {
            tv_sec: abs_target.as_secs() as i64,
            tv_nsec: abs_target.subsec_nanos() as i64,
        };
        let op = (FUTEX_WAIT_BITSET | FUTEX_CLOCK_REALTIME | FUTEX_PRIVATE_FLAG) as u64;
        let start = std::time::Instant::now();
        let res = unsafe {
            kinakaze_abi_syscall_raw(
                SYS_FUTEX,
                (&raw mut uaddr1) as u64,
                op,
                0,
                (&raw const abs_ts) as u64,
                0,
                FUTEX_BITSET_MATCH_ANY as u64,
            )
        };
        let elapsed = start.elapsed();
        assert_eq!(
            res,
            -i64::from(ETIMEDOUT),
            "FUTEX_WAIT_BITSET with absolute epoch timeout should return ETIMEDOUT"
        );
        assert!(
            elapsed >= std::time::Duration::from_millis(30)
                && elapsed < std::time::Duration::from_secs(2),
            "FUTEX_WAIT_BITSET absolute timeout took {:?} instead of 50ms",
            elapsed
        );

        // 3. FUTEX_WAKE wakes waiting thread
        let uaddr1_addr = (&raw mut uaddr1) as usize;
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_FUTEX,
                    uaddr1_addr as u64,
                    (FUTEX_WAKE | FUTEX_PRIVATE_FLAG) as u64,
                    1,
                    0,
                    0,
                    0,
                )
            };
        });
        let long_ts = TimeSpec {
            tv_sec: 5,
            tv_nsec: 0,
        };
        let start = std::time::Instant::now();
        let res = unsafe {
            kinakaze_abi_syscall_raw(
                SYS_FUTEX,
                (&raw mut uaddr1) as u64,
                (FUTEX_WAIT | FUTEX_PRIVATE_FLAG) as u64,
                0,
                (&raw const long_ts) as u64,
                0,
                0,
            )
        };
        let elapsed = start.elapsed();
        assert_eq!(res, 0, "FUTEX_WAKE should wake waiter with res=0");
        assert!(elapsed < std::time::Duration::from_secs(2));
        handle.join().unwrap();

        // 4. FUTEX_CMP_REQUEUE reports zero when no waiter can be transferred.
        let requeue_res = unsafe {
            kinakaze_abi_syscall_raw(
                SYS_FUTEX,
                (&raw mut uaddr1) as u64,
                (FUTEX_CMP_REQUEUE | FUTEX_PRIVATE_FLAG) as u64,
                1,
                1,
                (&raw mut uaddr2) as u64,
                0, // expected val = 0
            )
        };
        assert_eq!(requeue_res, 0);

        // Omitting PRIVATE_FLAG is valid for stack/heap/private-mapped words.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_FUTEX,
                    (&raw mut uaddr1) as u64,
                    FUTEX_WAKE as u64,
                    1,
                    0,
                    0,
                    0,
                )
            },
            0
        );
    }

    #[test]
    fn futex_expired_timeout_still_compares_value_first() {
        let word = AtomicI32::new(7);
        let expired = KernelTimespec::default();
        assert_eq!(
            futex_wait(word.as_ptr(), 0, &expired, true, false, u32::MAX),
            -i64::from(EAGAIN)
        );
        assert_eq!(
            futex_wait(word.as_ptr(), 7, &expired, true, false, u32::MAX),
            -i64::from(ETIMEDOUT)
        );
    }

    fn wait_for_futex_queue(address: usize, count: usize) {
        let started = std::time::Instant::now();
        loop {
            if futex_queues()
                .lock()
                .unwrap()
                .get(&address)
                .map_or(0, VecDeque::len)
                == count
            {
                return;
            }
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "futex queue did not reach {count} entries"
            );
            std::thread::yield_now();
        }
    }

    fn wake_op(first: &AtomicI32, second: &AtomicI32, counts: (u32, u32), encoded: u32) -> i64 {
        futex_syscall(
            first.as_ptr(),
            (FUTEX_WAKE_OP | FUTEX_PRIVATE_FLAG) as i32,
            counts.0,
            counts.1 as usize as *const KernelTimespec,
            second.as_ptr(),
            encoded,
        )
    }

    fn encoded_wake_op(operation: u32, operand: i32, comparison: u32, value: i32) -> u32 {
        operation << 28 | comparison << 24 | (operand as u32 & 0xfff) << 12 | (value as u32 & 0xfff)
    }

    #[test]
    fn futex_wake_op_atomic_operations_and_signed_operands() {
        let first = AtomicI32::new(0);
        let second = AtomicI32::new(0);
        for (operation, operand, before, after) in [
            (0, -2048, 17, -2048),
            (1, -1, 17, 16),
            (1, 1, i32::MAX, i32::MIN),
            (2, 0x30, 5, 0x35),
            (3, 3, 7, 4),
            (4, -1, 0x1234, !0x1234),
            (8, 31, 0, i32::MIN),
            (10, 33, 1, 3),
            (12, -1, 0, i32::MIN),
        ] {
            second.store(before, Ordering::SeqCst);
            assert_eq!(
                wake_op(
                    &first,
                    &second,
                    (1, 1),
                    encoded_wake_op(operation, operand, 0, 0)
                ),
                0
            );
            assert_eq!(second.load(Ordering::SeqCst), after);
        }
        second.store(7, Ordering::SeqCst);
        assert_eq!(
            wake_op(&first, &second, (1, 1), encoded_wake_op(5, 2, 0, 0)),
            -i64::from(ENOSYS)
        );
        assert_eq!(second.load(Ordering::SeqCst), 7);
        // An invalid comparison fails after the atomic operation, as in Linux.
        assert_eq!(
            wake_op(&first, &second, (1, 1), encoded_wake_op(1, 2, 6, 0)),
            -i64::from(ENOSYS)
        );
        assert_eq!(second.load(Ordering::SeqCst), 9);
        assert_eq!(
            futex_syscall(
                first.as_ptr(),
                (FUTEX_WAKE_OP | FUTEX_PRIVATE_FLAG) as i32,
                1,
                core::ptr::null(),
                core::ptr::null_mut(),
                0
            ),
            -i64::from(EFAULT)
        );
        assert_eq!(
            futex_syscall(
                first.as_ptr(),
                (FUTEX_WAKE_OP | FUTEX_PRIVATE_FLAG) as i32,
                1,
                core::ptr::null(),
                1usize as *mut i32,
                0
            ),
            -i64::from(EINVAL)
        );
    }

    fn spawn_futex_waiter(word: &Arc<AtomicI32>) -> std::thread::JoinHandle<i64> {
        let word = Arc::clone(word);
        std::thread::spawn(move || {
            let timeout = KernelTimespec {
                tv_sec: 5,
                tv_nsec: 0,
            };
            futex_wait(
                word.as_ptr(),
                word.load(Ordering::SeqCst),
                &timeout,
                false,
                false,
                u32::MAX,
            )
        })
    }

    #[test]
    fn futex_wake_op_compares_old_signed_value_and_wakes_both_queues() {
        for comparison in 0..6 {
            for old in [-2, -1, 0] {
                let first = Arc::new(AtomicI32::new(0));
                let second = Arc::new(AtomicI32::new(old));
                let a = spawn_futex_waiter(&first);
                let b = spawn_futex_waiter(&second);
                wait_for_futex_queue(first.as_ptr() as usize, 1);
                wait_for_futex_queue(second.as_ptr() as usize, 1);
                let matches = match comparison {
                    0 => old == -1,
                    1 => old != -1,
                    2 => old < -1,
                    3 => old <= -1,
                    4 => old > -1,
                    _ => old >= -1,
                };
                let woken = wake_op(
                    &first,
                    &second,
                    (1, 1),
                    encoded_wake_op(0, 19, comparison, -1),
                );
                // Clean up before assertions so a failed case leaves no waiter.
                let remaining_a = futex_wake(first.as_ptr(), u32::MAX, u32::MAX);
                let remaining_b = futex_wake(second.as_ptr(), u32::MAX, u32::MAX);
                assert_eq!(a.join().unwrap(), 0);
                assert_eq!(b.join().unwrap(), 0);
                assert_eq!(woken, 1 + i64::from(matches));
                assert_eq!(remaining_a, 0);
                assert_eq!(remaining_b, i64::from(!matches));
                assert_eq!(second.load(Ordering::SeqCst), 19);
            }
        }
    }

    #[test]
    fn futex_wake_op_same_address_obeys_both_wake_limits() {
        let word = Arc::new(AtomicI32::new(0));
        let threads: Vec<_> = (0..3).map(|_| spawn_futex_waiter(&word)).collect();
        wait_for_futex_queue(word.as_ptr() as usize, 3);
        let woken = wake_op(&word, &word, (1, 1), encoded_wake_op(0, 7, 0, 0));
        let remaining = futex_wake(word.as_ptr(), u32::MAX, u32::MAX);
        for thread in threads {
            assert_eq!(thread.join().unwrap(), 0);
        }
        assert_eq!(woken, 2);
        assert_eq!(remaining, 1);
        assert_eq!(word.load(Ordering::SeqCst), 7);
    }

    #[test]
    fn futex_bitsets_requeue_and_wake_are_not_spurious_success() {
        let source = Arc::new(AtomicI32::new(0));
        let destination = AtomicI32::new(0);
        let spawn = |bitset| {
            let word = Arc::clone(&source);
            std::thread::spawn(move || {
                let timeout = KernelTimespec {
                    tv_sec: 5,
                    tv_nsec: 0,
                };
                futex_wait(word.as_ptr(), 0, &timeout, false, false, bitset)
            })
        };
        let first = spawn(1);
        let second = spawn(2);
        wait_for_futex_queue(source.as_ptr() as usize, 2);
        assert_eq!(futex_wake(source.as_ptr(), 1, 4), 0);
        assert_eq!(futex_wake(source.as_ptr(), 1, 1), 1);
        assert_eq!(first.join().unwrap(), 0);
        assert_eq!(
            futex_syscall(
                source.as_ptr(),
                (FUTEX_CMP_REQUEUE | FUTEX_PRIVATE_FLAG) as i32,
                0,
                1usize as *const KernelTimespec,
                destination.as_ptr(),
                0
            ),
            1
        );
        assert_eq!(futex_wake(source.as_ptr(), 1, u32::MAX), 0);
        assert_eq!(futex_wake(destination.as_ptr(), 1, 2), 1);
        assert_eq!(second.join().unwrap(), 0);
    }

    #[test]
    fn futex_signals_interrupt_restart_and_unqueue_before_handler() {
        use kinakaze_vfs::{interrupt, signal};
        use std::sync::atomic::{AtomicBool, AtomicU32};
        static CALLS: AtomicU32 = AtomicU32::new(0);
        static ADDRESS: AtomicUsize = AtomicUsize::new(0);
        static HANDLER_WAKE: AtomicI32 = AtomicI32::new(-1);
        unsafe extern "sysv64" fn handler(_: i32) {
            HANDLER_WAKE.store(
                futex_wake(
                    ADDRESS.load(Ordering::Acquire) as *mut i32,
                    u32::MAX,
                    u32::MAX,
                ) as i32,
                Ordering::Release,
            );
            CALLS.fetch_add(1, Ordering::AcqRel);
        }
        let word = Arc::new(AtomicI32::new(0));
        ADDRESS.store(word.as_ptr() as usize, Ordering::Release);
        let action = |flags| signal::Action {
            disposition: signal::Disposition::Handle(handler, 0),
            flags,
            mask: 0,
            restorer: 0,
        };
        let previous = signal::sigaction(signal::SIGUSR2, Some(action(0))).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let target = Arc::clone(&word);
        let first = std::thread::spawn(move || {
            tx.send(interrupt::current_thread_id()).unwrap();
            let timeout = KernelTimespec {
                tv_sec: 5,
                tv_nsec: 0,
            };
            futex_wait(target.as_ptr(), 0, &timeout, false, false, u32::MAX)
        });
        let tid = rx.recv().unwrap();
        wait_for_futex_queue(word.as_ptr() as usize, 1);
        signal::raise_thread_signal(tid, signal::SIGUSR2).unwrap();
        assert_eq!(first.join().unwrap(), -i64::from(kinakaze_vfs::EINTR));
        assert_eq!(
            HANDLER_WAKE.load(Ordering::Acquire),
            0,
            "outer wait must not be visible to its handler"
        );
        assert_eq!(CALLS.load(Ordering::Acquire), 1);

        signal::sigaction(signal::SIGUSR2, Some(action(signal::SA_RESTART))).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let ready = Arc::new(AtomicBool::new(false));
        let target_ready = Arc::clone(&ready);
        let target = Arc::clone(&word);
        let second = std::thread::spawn(move || {
            tx.send(interrupt::current_thread_id()).unwrap();
            while !target_ready.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            // Signal queued before event creation / waiter registration must
            // still run, without consuming or renewing the timeout budget.
            let timeout = KernelTimespec {
                tv_sec: 0,
                tv_nsec: 100_000_000,
            };
            futex_wait(target.as_ptr(), 0, &timeout, false, false, u32::MAX)
        });
        signal::raise_thread_signal(rx.recv().unwrap(), signal::SIGUSR2).unwrap();
        ready.store(true, Ordering::Release);
        assert_eq!(second.join().unwrap(), -i64::from(ETIMEDOUT));
        assert_eq!(CALLS.load(Ordering::Acquire), 2);
        assert_eq!(HANDLER_WAKE.load(Ordering::Acquire), 0);
        signal::sigaction(signal::SIGUSR2, Some(previous)).unwrap();
        wait_for_futex_queue(word.as_ptr() as usize, 0);
    }

    #[test]
    fn syscall_reports_errors_the_way_glibc_does() {
        // `syscall` is the libc entry point, so it owes callers glibc's
        // contract: -1 with errno set. Node.js died on this exact distinction —
        // libuv tests `if (ringfd == -1)` after `io_uring_setup`, and a raw
        // -ENOSYS sailed past that guard and reached `uv__close` as if it were
        // a descriptor number.
        // SAFETY: an unrecognised number dereferences nothing.
        let result = unsafe { kinakaze_abi_syscall(999_999, 0, 0, 0, 0, 0, 0) };
        assert_eq!(result, -1, "a failed syscall must return -1");
        assert_eq!(kinakaze_tls::errno(), ENOSYS, "and report the errno");

        // SAFETY: syslog ignores its arguments here.
        assert_eq!(
            unsafe { kinakaze_abi_syscall(SYS_SYSLOG, 0, 0, 0, 0, 0, 0) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EPERM);
    }

    #[test]
    fn syscall_leaves_successful_high_addresses_alone() {
        // A large `mmap` result can be negative when read as i64. Only the
        // kernel's error band (-4095..0) is an error; anything else must pass
        // through untouched rather than being flattened to -1.
        // SAFETY: no memory is touched, only the return value is inspected.
        let ok = unsafe { kinakaze_abi_syscall(SYS_GETTID, 0, 0, 0, 0, 0, 0) };
        assert!(ok > 0, "gettid must succeed, got {ok}");
        assert_ne!(ok, -1);
    }

    #[test]
    fn syscall_gettid_reports_a_plausible_thread_id() {
        // SAFETY: gettid takes no arguments.
        let tid = unsafe { kinakaze_abi_syscall_raw(SYS_GETTID, 0, 0, 0, 0, 0, 0) };
        assert!(tid > 0, "a thread id must be positive, got {tid}");
        assert_eq!(
            tid,
            i64::from(current_tid()),
            "the raw path and the wrapper must agree"
        );

        // Stable within a thread, and distinct between threads.
        // SAFETY: gettid takes no arguments.
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(SYS_GETTID, 0, 0, 0, 0, 0, 0) },
            tid
        );
        let other = std::thread::spawn(|| {
            // SAFETY: gettid takes no arguments.
            unsafe { kinakaze_abi_syscall_raw(SYS_GETTID, 0, 0, 0, 0, 0, 0) }
        })
        .join()
        .expect("the probe thread panicked");
        assert_ne!(other, tid, "two live threads must have different ids");

        // SAFETY: getpid takes no arguments.
        let pid = unsafe { kinakaze_abi_syscall_raw(SYS_GETPID, 0, 0, 0, 0, 0, 0) };
        assert_eq!(pid, i64::from(crate::process::kinakaze_abi_getpid()));
    }

    #[test]
    fn syscall_getrandom_fills_the_buffer() {
        let mut buffer = [0u8; 64];
        // SAFETY: `buffer` is 64 writable bytes.
        let count = unsafe {
            kinakaze_abi_syscall_raw(
                SYS_GETRANDOM,
                buffer.as_mut_ptr() as u64,
                buffer.len() as u64,
                0,
                0,
                0,
                0,
            )
        };
        assert_eq!(
            count,
            buffer.len() as i64,
            "the whole buffer must be filled"
        );
        // Not a proof of randomness, but it does catch the failure that matters:
        // a stub that returned success without writing anything.
        assert!(
            buffer.iter().any(|byte| *byte != 0),
            "64 bytes of zeros means nothing was written"
        );

        // A zero-length request asks for nothing and is well-formed.
        // SAFETY: no buffer is read at length zero.
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(SYS_GETRANDOM, 0, 0, 0, 0, 0, 0) },
            0
        );

        // An unknown flag is EINVAL, so a caller using a newer flag learns it was
        // not honoured. Negated, per the raw convention.
        // SAFETY: the call fails on the flags before touching the buffer.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_GETRANDOM,
                    buffer.as_mut_ptr() as u64,
                    buffer.len() as u64,
                    0x8000,
                    0,
                    0,
                    0,
                )
            },
            -i64::from(EINVAL)
        );
    }

    #[test]
    fn syscall_membarrier_advertises_only_what_it_serves() {
        // SAFETY: the QUERY command reads no pointer.
        let supported = unsafe {
            kinakaze_abi_syscall_raw(SYS_MEMBARRIER, MEMBARRIER_CMD_QUERY as u64, 0, 0, 0, 0, 0)
        };
        assert!(supported > 0, "QUERY must report a non-empty mask");

        // Every advertised command must actually work. A mask promising a command
        // that then fails is worse than a narrower mask.
        for command in [
            MEMBARRIER_CMD_GLOBAL,
            MEMBARRIER_CMD_GLOBAL_EXPEDITED,
            MEMBARRIER_CMD_PRIVATE_EXPEDITED,
            MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED,
            MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED,
        ] {
            assert_ne!(
                supported & command,
                0,
                "command {command} must be advertised"
            );
            // SAFETY: no command reads a pointer.
            assert_eq!(
                unsafe { kinakaze_abi_syscall_raw(SYS_MEMBARRIER, command as u64, 0, 0, 0, 0, 0) },
                0,
                "advertised command {command} must succeed"
            );
        }

        // An unknown command and a non-zero flags word are both EINVAL.
        // SAFETY: no pointer is read.
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(SYS_MEMBARRIER, 1 << 20, 0, 0, 0, 0, 0) },
            -i64::from(EINVAL)
        );
        // SAFETY: no pointer is read.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_MEMBARRIER,
                    MEMBARRIER_CMD_GLOBAL as u64,
                    1,
                    0,
                    0,
                    0,
                    0,
                )
            },
            -i64::from(EINVAL)
        );
    }

    #[test]
    fn syscall_sched_getaffinity_reports_bytes_written() {
        // The raw form differs from the wrapper: the kernel reports how much of the
        // cpumask it filled, where glibc reports 0. Conflating them would have a
        // caller read a byte count as an error or a 0 as an empty mask.
        let mut set = [0u8; 128];
        // SAFETY: `set` is 128 writable bytes.
        let result = unsafe {
            kinakaze_abi_syscall_raw(
                SYS_SCHED_GETAFFINITY,
                0,
                set.len() as u64,
                set.as_mut_ptr() as u64,
                0,
                0,
                0,
            )
        };
        assert_eq!(
            result,
            size_of::<usize>() as i64,
            "the raw call reports the bytes written"
        );
        assert_ne!(set[..size_of::<usize>()], [0u8; size_of::<usize>()]);

        // Errors are negated rather than -1.
        // SAFETY: the call fails on the size before touching the buffer.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_SCHED_GETAFFINITY,
                    0,
                    4,
                    set.as_mut_ptr() as u64,
                    0,
                    0,
                    0,
                )
            },
            -i64::from(EINVAL)
        );
    }

    #[test]
    fn syscall_clock_gettime_answers_the_clocks_windows_has() {
        let mut now = KernelTimespec::default();
        // SAFETY: `now` is a writable local.
        let result = unsafe {
            kinakaze_abi_syscall_raw(
                SYS_CLOCK_GETTIME,
                CLOCK_REALTIME as u64,
                (&raw mut now) as u64,
                0,
                0,
                0,
                0,
            )
        };
        assert_eq!(result, 0);
        // A real Unix timestamp, not a tick count: 1.7e9 is 2023, so anything below
        // it means the 1601 epoch was not rebased.
        assert!(
            now.tv_sec > 1_700_000_000,
            "CLOCK_REALTIME must be Unix-epoch seconds, got {}",
            now.tv_sec
        );
        assert!((0..1_000_000_000).contains(&now.tv_nsec));

        // The monotonic clocks must not run backwards.
        let mut first = KernelTimespec::default();
        // SAFETY: `first` is a writable local.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_CLOCK_GETTIME,
                    CLOCK_MONOTONIC as u64,
                    (&raw mut first) as u64,
                    0,
                    0,
                    0,
                    0,
                )
            },
            0
        );
        let mut second = KernelTimespec::default();
        // SAFETY: `second` is a writable local.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_CLOCK_GETTIME,
                    CLOCK_MONOTONIC as u64,
                    (&raw mut second) as u64,
                    0,
                    0,
                    0,
                    0,
                )
            },
            0
        );
        let elapsed =
            (second.tv_sec - first.tv_sec) * 1_000_000_000 + (second.tv_nsec - first.tv_nsec);
        assert!(
            elapsed >= 0,
            "CLOCK_MONOTONIC went backwards by {elapsed} ns"
        );
        assert!((0..1_000_000_000).contains(&second.tv_nsec));

        // The CPU clocks are real, and this process has consumed some CPU by now.
        let mut cpu = KernelTimespec::default();
        // SAFETY: `cpu` is a writable local.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_CLOCK_GETTIME,
                    CLOCK_PROCESS_CPUTIME_ID as u64,
                    (&raw mut cpu) as u64,
                    0,
                    0,
                    0,
                    0,
                )
            },
            0
        );
        assert!(cpu.tv_sec >= 0 && (0..1_000_000_000).contains(&cpu.tv_nsec));

        // An unknown clock is EINVAL, negated.
        // SAFETY: `now` is a writable local; the call fails before writing.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(SYS_CLOCK_GETTIME, 99, (&raw mut now) as u64, 0, 0, 0, 0)
            },
            -i64::from(EINVAL)
        );
        // A null timespec is EFAULT rather than a crash.
        // SAFETY: passing null is the case under test.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(SYS_CLOCK_GETTIME, CLOCK_MONOTONIC as u64, 0, 0, 0, 0, 0)
            },
            -i64::from(EFAULT)
        );
    }

    #[test]
    fn syscall_set_tid_address_returns_the_tid() {
        let mut slot = 0i32;
        // SAFETY: `slot` is a writable local, though this never writes through it.
        let result = unsafe {
            kinakaze_abi_syscall_raw(SYS_SET_TID_ADDRESS, (&raw mut slot) as u64, 0, 0, 0, 0, 0)
        };
        assert_eq!(
            result,
            i64::from(current_tid()),
            "set_tid_address returns the caller's tid, which glibc uses"
        );
    }

    #[test]
    fn raw_signal_syscalls_use_the_32_byte_kernel_abi() {
        unsafe extern "sysv64" fn handler(_signal: i32) {}

        const SIGNAL: u64 = 62;
        const RESTORER: usize = 0x1234_5678;
        const CANARY: u64 = 0xfeed_face_cafe_beef;

        let requested = KernelSigAction {
            sa_handler: handler as usize,
            sa_flags: kinakaze_vfs::signal::SA_RESTART as u64,
            sa_restorer: RESTORER,
            sa_mask: 1 << (SIGNAL - 1),
        };
        #[repr(C)]
        struct GuardedAction {
            action: KernelSigAction,
            canary: u64,
        }
        let mut old = GuardedAction {
            action: KernelSigAction::default(),
            canary: CANARY,
        };
        // SAFETY: both action records are live and the kernel ABI mask size is 8.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_RT_SIGACTION,
                    SIGNAL,
                    (&raw const requested) as u64,
                    (&raw mut old.action) as u64,
                    size_of::<u64>() as u64,
                    0,
                    0,
                )
            },
            0
        );
        assert_eq!(old.canary, CANARY, "rt_sigaction wrote past 32 bytes");

        let mut queried = GuardedAction {
            action: KernelSigAction::default(),
            canary: CANARY,
        };
        // SAFETY: the output record is writable and action is null for a query.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_RT_SIGACTION,
                    SIGNAL,
                    0,
                    (&raw mut queried.action) as u64,
                    size_of::<u64>() as u64,
                    0,
                    0,
                )
            },
            0
        );
        assert_eq!(queried.canary, CANARY);
        assert_eq!(queried.action.sa_handler, handler as usize);
        assert_eq!(queried.action.sa_flags, requested.sa_flags);
        assert_eq!(queried.action.sa_restorer, RESTORER);
        assert_eq!(queried.action.sa_mask, requested.sa_mask);

        let mut previous_mask = 0u64;
        let block = requested.sa_mask;
        // SAFETY: both mask pointers name exactly one kernel-sized 64-bit word.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_RT_SIGPROCMASK,
                    kinakaze_vfs::signal::SIG_BLOCK as u64,
                    (&raw const block) as u64,
                    (&raw mut previous_mask) as u64,
                    size_of::<u64>() as u64,
                    0,
                    0,
                )
            },
            0
        );
        let mut observed = 0u64;
        // Linux ignores `how` on a null-set query.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_RT_SIGPROCMASK,
                    u64::MAX,
                    0,
                    (&raw mut observed) as u64,
                    size_of::<u64>() as u64,
                    0,
                    0,
                )
            },
            0
        );
        assert_ne!(observed & block, 0);
        // SAFETY: restore the per-thread mask captured above.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_RT_SIGPROCMASK,
                    kinakaze_vfs::signal::SIG_SETMASK as u64,
                    (&raw const previous_mask) as u64,
                    0,
                    size_of::<u64>() as u64,
                    0,
                    0,
                )
            },
            0
        );

        let default = KernelSigAction::default();
        // SAFETY: restore the signal disposition for the rest of the suite.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_RT_SIGACTION,
                    SIGNAL,
                    (&raw const default) as u64,
                    0,
                    size_of::<u64>() as u64,
                    0,
                    0,
                )
            },
            0
        );

        // The fourth argument is not advisory: x86_64 Linux accepts exactly 8.
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(SYS_RT_SIGACTION, SIGNAL, 0, 0, 128, 0, 0) },
            -i64::from(EINVAL)
        );
    }

    #[test]
    fn filesystem_administration_tracks_bind_mounts_and_rejects_absent_mounts() {
        clear_errno();
        assert_eq!(
            unsafe {
                kinakaze_abi_mount(
                    c"proc".as_ptr(),
                    c"/proc".as_ptr(),
                    c"proc".as_ptr(),
                    0,
                    core::ptr::null(),
                )
            },
            0
        );
        assert_eq!(unsafe { kinakaze_abi_umount2(c"/proc".as_ptr(), 0) }, 0);

        clear_errno();
        assert_eq!(unsafe { kinakaze_abi_umount2(c"/mnt".as_ptr(), 0) }, -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let source_guest = format!("/kinakaze-mount-source-{nonce}");
        let target_guest = format!("/kinakaze-mount-target-{nonce}");
        let root = kinakaze_vfs::system_root().unwrap();
        let source_host = root.join(source_guest.trim_start_matches('/'));
        let target_host = root.join(target_guest.trim_start_matches('/'));
        std::fs::create_dir(&source_host).unwrap();
        std::fs::create_dir(&target_host).unwrap();
        std::fs::write(source_host.join("visible"), b"mounted").unwrap();
        let source = std::ffi::CString::new(source_guest.clone()).unwrap();
        let target = std::ffi::CString::new(target_guest.clone()).unwrap();
        assert_eq!(
            unsafe {
                kinakaze_abi_mount(
                    source.as_ptr(),
                    target.as_ptr(),
                    c"bind".as_ptr(),
                    kinakaze_vfs::mount::MS_BIND,
                    core::ptr::null(),
                )
            },
            0
        );
        assert_eq!(
            std::fs::read(
                kinakaze_vfs::resolve_linux_path(&format!("{target_guest}/visible")).unwrap()
            )
            .unwrap(),
            b"mounted"
        );
        assert_eq!(unsafe { kinakaze_abi_umount2(target.as_ptr(), 0) }, 0);
        clear_errno();
        assert_eq!(unsafe { kinakaze_abi_umount2(target.as_ptr(), 0) }, -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        std::fs::remove_dir_all(source_host).unwrap();
        std::fs::remove_dir(target_host).unwrap();

        clear_errno();
        // Swapon/swapoff remain ENOSYS as Windows handles paging globally.
        assert_eq!(unsafe { kinakaze_abi_swapon(c"/swapfile".as_ptr(), 0) }, -1);
        assert_eq!(kinakaze_tls::errno(), ENOSYS);

        clear_errno();
        assert_eq!(unsafe { kinakaze_abi_swapoff(c"/swapfile".as_ptr()) }, -1);
        assert_eq!(kinakaze_tls::errno(), ENOSYS);
    }

    #[test]
    fn namespace_arguments_reject_invalid_flags_and_descriptors() {
        clear_errno();
        assert_eq!(kinakaze_abi_unshare(0), 0);
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(SYS_UNSHARE, 0x8000_0000, 0, 0, 0, 0, 0) },
            -i64::from(EINVAL)
        );
        assert_eq!(kinakaze_abi_unshare(-1), -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        clear_errno();
        assert_eq!(kinakaze_abi_setns(-1, 0), -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EBADF);
        let fd = kinakaze_vfs::fs::open("/dev/null", 0, 0).unwrap();
        assert_eq!(kinakaze_abi_setns(fd, 0), -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        kinakaze_vfs::close(fd).unwrap();
    }

    #[test]
    fn cgroup_filesystem_mount_publishes_a_real_superblock() {
        clear_errno();
        assert_eq!(
            unsafe {
                kinakaze_abi_mount(
                    c"cgroup2".as_ptr(),
                    c"/sys/fs/cgroup".as_ptr(),
                    c"cgroup2".as_ptr(),
                    0,
                    core::ptr::null(),
                )
            },
            0
        );
        assert_eq!(
            kinakaze_vfs::tmpfs::statfs("/sys/fs/cgroup")
                .unwrap()
                .unwrap()
                .magic,
            0x63677270
        );
        let fd = kinakaze_vfs::fs::open("/sys/fs/cgroup/cgroup.controllers", 0, 0).unwrap();
        let mut text = [0; 128];
        assert!(kinakaze_vfs::read(fd, &mut text).unwrap() > 0);
        kinakaze_vfs::close(fd).unwrap();
        assert_eq!(
            unsafe { kinakaze_abi_umount2(c"/sys/fs/cgroup".as_ptr(), 0) },
            0
        );
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_MOUNT,
                    c"cgroup2".as_ptr() as u64,
                    c"/sys/fs/cgroup".as_ptr() as u64,
                    c"cgroup2".as_ptr() as u64,
                    0,
                    0,
                    0,
                )
            },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_umount2(c"/sys/fs/cgroup".as_ptr(), 0) },
            0
        );
    }

    #[test]
    fn container_primitives_seccomp_and_memfd_create() {
        clear_errno();
        assert_eq!(
            unsafe { kinakaze_abi_seccomp(1, 0, core::ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::ENOSYS);

        clear_errno();
        let fd = unsafe { kinakaze_abi_memfd_create(c"test_memfd".as_ptr(), 0) };
        assert!(fd >= 0, "memfd_create failed: fd={fd}");
        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn container_startup_sequence_and_virtualization_invariants() {
        // Container namespace combinations create distinct objects and can be
        // restored using the original descriptors in the current PID namespace.
        let original: Vec<_> = ["mnt", "pid", "net", "ipc", "uts"]
            .into_iter()
            .map(|name| kinakaze_vfs::fs::open(&format!("/proc/self/ns/{name}"), 0, 0).unwrap())
            .collect();
        const CLONE_NEWNS: c_int = 0x0002_0000;
        const CLONE_NEWPID: c_int = 0x2000_0000;
        const CLONE_NEWNET: c_int = 0x4000_0000;
        const CLONE_NEWIPC: c_int = 0x0800_0000;
        const CLONE_NEWUTS: c_int = 0x0400_0000;
        assert_eq!(
            kinakaze_abi_unshare(
                CLONE_NEWNS | CLONE_NEWPID | CLONE_NEWNET | CLONE_NEWIPC | CLONE_NEWUTS
            ),
            0
        );
        for fd in original {
            assert_eq!(kinakaze_abi_setns(fd, 0), 0);
            kinakaze_vfs::close(fd).unwrap();
        }

        // The mount now publishes a backed cgroup superblock.
        assert_eq!(
            unsafe {
                kinakaze_abi_mount(
                    c"cgroup2".as_ptr(),
                    c"/sys/fs/cgroup".as_ptr(),
                    c"cgroup2".as_ptr(),
                    0,
                    core::ptr::null(),
                )
            },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_umount2(c"/sys/fs/cgroup".as_ptr(), 0) },
            0
        );

        // 3. Security confinements: no_new_privs & capability checks
        assert_eq!(
            unsafe { kinakaze_abi_prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) },
            1
        );
        // CAP_SYS_ADMIN = 21, CAP_NET_ADMIN = 12
        assert_eq!(
            unsafe { kinakaze_abi_prctl(PR_CAPBSET_READ, 21, 0, 0, 0) },
            1
        );
        assert_eq!(
            unsafe { kinakaze_abi_prctl(PR_CAPBSET_READ, 12, 0, 0, 0) },
            1
        );

        // 4. Unsupported seccomp cannot claim successful confinement.
        assert_eq!(
            unsafe { kinakaze_abi_seccomp(0, 0, core::ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::ENOSYS);

        // 5. Cgroup resource management under /sys/fs/cgroup
        assert_eq!(
            kinakaze_vfs::cgroup::create_directory("/sys/fs/cgroup/container-init"),
            Ok(())
        );
        assert_eq!(
            kinakaze_vfs::cgroup::write_file(
                "/sys/fs/cgroup/container-init/cpu.max",
                b"20000 100000"
            ),
            Ok(())
        );
        assert_eq!(
            kinakaze_vfs::cgroup::write_file(
                "/sys/fs/cgroup/container-init/memory.max",
                b"134217728"
            ),
            Ok(())
        );

        let cpu_limit =
            kinakaze_vfs::cgroup::read_file("/sys/fs/cgroup/container-init/cpu.max").unwrap();
        assert_eq!(std::str::from_utf8(&cpu_limit).unwrap(), "20000 100000\n");

        let mem_limit =
            kinakaze_vfs::cgroup::read_file("/sys/fs/cgroup/container-init/memory.max").unwrap();
        assert_eq!(std::str::from_utf8(&mem_limit).unwrap(), "134217728\n");

        assert_eq!(
            kinakaze_vfs::cgroup::remove_directory("/sys/fs/cgroup/container-init"),
            Ok(())
        );
    }

    #[test]
    fn overlay_mount_copies_up_and_preserves_lower_and_covered_target() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kinakaze-overlay-mount-rejection-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let target = kinakaze_vfs::to_guest_path(&self.0.join("target"));
                let _ = kinakaze_vfs::mount::unmount(&target, 2);
                // This test exclusively created the root and every child.
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let fixture = Fixture(root);
        for name in ["lower", "upper", "work", "target"] {
            std::fs::create_dir(fixture.0.join(name)).unwrap();
        }
        std::fs::write(fixture.0.join("lower/data"), b"immutable lower").unwrap();
        std::fs::write(fixture.0.join("target/original"), b"target retained").unwrap();
        let target_guest = kinakaze_vfs::to_guest_path(&fixture.0.join("target"));
        let target = std::ffi::CString::new(target_guest.clone()).unwrap();
        let data = std::ffi::CString::new(format!(
            "lowerdir={},upperdir={},workdir={}",
            kinakaze_vfs::to_guest_path(&fixture.0.join("lower")),
            kinakaze_vfs::to_guest_path(&fixture.0.join("upper")),
            kinakaze_vfs::to_guest_path(&fixture.0.join("work")),
        ))
        .unwrap();
        let source = c"overlay".as_ptr();
        let fstype = c"overlay".as_ptr();
        assert_eq!(
            unsafe { kinakaze_abi_mount(source, target.as_ptr(), fstype, 0, data.as_ptr().cast()) },
            0
        );
        let bad_options = std::ffi::CString::new(format!(
            "{},index=off,nfs_export=on",
            data.to_str().unwrap()
        ))
        .unwrap();
        let before = kinakaze_vfs::mount::snapshot_list().unwrap();
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_MOUNT,
                    source as u64,
                    target.as_ptr() as u64,
                    fstype as u64,
                    0,
                    bad_options.as_ptr() as u64,
                    0,
                )
            },
            -i64::from(kinakaze_vfs::EINVAL)
        );
        assert_eq!(kinakaze_vfs::mount::snapshot_list().unwrap(), before);
        let fd =
            kinakaze_vfs::fs::open(&format!("{target_guest}/data"), kinakaze_vfs::fs::O_RDWR, 0)
                .unwrap();
        kinakaze_vfs::write(fd, b"UPPER").unwrap();
        kinakaze_vfs::close(fd).unwrap();
        assert_eq!(
            std::fs::read(fixture.0.join("lower/data")).unwrap(),
            b"immutable lower"
        );
        assert_eq!(
            std::fs::read(fixture.0.join("target/original")).unwrap(),
            b"target retained"
        );
        assert!(
            std::fs::read(fixture.0.join("upper/data"))
                .unwrap()
                .starts_with(b"UPPER")
        );
        assert_eq!(
            std::fs::read_dir(fixture.0.join("work")).unwrap().count(),
            0
        );
        assert_eq!(unsafe { umount2(target.as_ptr(), 0) }, 0);
        assert_eq!(unsafe { umount2(target.as_ptr(), 0) }, -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn personality_answers_the_query_and_refuses_other_domains() {
        // The query form returns the current persona, and 0 is a success here, not
        // an error: the return value *is* the previous persona.
        clear_errno();
        assert_eq!(kinakaze_abi_personality(PERSONALITY_QUERY), 0);
        assert_eq!(kinakaze_tls::errno(), 0, "a query must not set errno");

        // Asking for the persona already in force succeeds.
        clear_errno();
        assert_eq!(kinakaze_abi_personality(PER_LINUX), 0);

        // ADDR_NO_RANDOMIZE, which `setarch -R` sets. Refused, because the guest is
        // already mapped by the time any guest code runs and there is no later
        // layout decision left to influence.
        clear_errno();
        assert_eq!(kinakaze_abi_personality(0x0004_0000), -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn privileged_control_refuses_deliberately() {
        // chroot: missing directory reports ENOENT.
        clear_errno();
        assert_eq!(
            unsafe { kinakaze_abi_chroot(c"/nonexistent_jail_dir".as_ptr()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::ENOENT);

        // klogctl: EPERM. `dmesg` calls this and will report the failure, which is
        // the correct outcome — there is no kernel ring buffer here to read.
        clear_errno();
        let mut buffer = [0u8; 64];
        // SAFETY: the buffer is writable and is not touched on this path.
        assert_eq!(
            unsafe {
                kinakaze_abi_klogctl(
                    3,
                    buffer.as_mut_ptr().cast::<c_char>(),
                    buffer.len() as c_int,
                )
            },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EPERM);
    }

    #[test]
    fn reboot_validates_its_command_and_then_refuses() {
        // This test never asks for a reboot that could happen: no shutdown API is
        // declared in this module, so there is nothing for a valid command to reach.
        // What is asserted is that the refusal is EPERM and the validation runs
        // first, so a caller gets the error its request actually earned.
        clear_errno();
        assert_eq!(kinakaze_abi_reboot(0x1234), -1);
        assert_eq!(
            kinakaze_tls::errno(),
            EINVAL,
            "an unrecognised command is rejected before the refusal"
        );

        // A well-formed request is refused, not performed.
        for command in [
            LINUX_REBOOT_CMD_RESTART,
            LINUX_REBOOT_CMD_HALT,
            LINUX_REBOOT_CMD_POWER_OFF,
            LINUX_REBOOT_CMD_CAD_OFF,
        ] {
            clear_errno();
            assert_eq!(kinakaze_abi_reboot(command), -1, "command {command:#x}");
            assert_eq!(
                kinakaze_tls::errno(),
                EPERM,
                "a compatibility layer must not reboot the host"
            );
        }

        // On the raw path the magic numbers are supplied and are checked first, as
        // the kernel orders it.
        // SAFETY: reboot dereferences nothing.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_REBOOT,
                    0xdead_beef,
                    u64::from(LINUX_REBOOT_MAGIC2),
                    LINUX_REBOOT_CMD_RESTART as u64,
                    0,
                    0,
                    0,
                )
            },
            -i64::from(EINVAL),
            "a bad magic pair is EINVAL before any privilege question"
        );
        // Correct magic, understood command: still refused, and negated.
        // SAFETY: reboot dereferences nothing.
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_REBOOT,
                    u64::from(LINUX_REBOOT_MAGIC1),
                    u64::from(LINUX_REBOOT_MAGIC2),
                    LINUX_REBOOT_CMD_RESTART as u64,
                    0,
                    0,
                    0,
                )
            },
            -i64::from(EPERM)
        );
        // Every one of the four accepted second magics is recognised.
        for magic in [
            LINUX_REBOOT_MAGIC2,
            LINUX_REBOOT_MAGIC2A,
            LINUX_REBOOT_MAGIC2B,
            LINUX_REBOOT_MAGIC2C,
        ] {
            assert!(reboot_magic_is_valid(LINUX_REBOOT_MAGIC1, magic));
        }
        assert!(!reboot_magic_is_valid(0, LINUX_REBOOT_MAGIC2));
    }

    #[test]
    fn kernel_convention_conversion_is_not_the_libc_one() {
        // to_kernel is the hinge the whole gate turns on, so it is pinned directly.
        assert_eq!(to_kernel(0), 0);
        assert_eq!(to_kernel(42), 42);
        crate::set_errno(EPERM);
        assert_eq!(to_kernel(-1), -i64::from(EPERM));
        // A libc function that failed without setting errno still must not look
        // like a success.
        crate::set_errno(0);
        assert_eq!(to_kernel(-1), -i64::from(kinakaze_vfs::EIO));
    }

    #[test]
    fn struct_layouts_match_the_linux_abi() {
        // Guest code reads these offsets directly.
        assert_eq!(size_of::<RLimit64>(), 16);
        assert_eq!(align_of::<RLimit64>(), 8);
        assert_eq!(size_of::<KernelTimespec>(), 16);
        assert_eq!(align_of::<KernelTimespec>(), 8);
        assert_eq!(size_of::<KernelSigAction>(), 32);
        assert_eq!(align_of::<KernelSigAction>(), 8);
        // The Windows structs this module passes to Win32 must match the headers or
        // the calls corrupt memory.
        assert_eq!(size_of::<FileTime>(), 8);
    }
}

#[repr(C)]
pub struct SchedParam {
    pub sched_priority: c_int,
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sched_getparam(
    _pid: c_int,
    param: *mut SchedParam,
) -> c_int {
    if !param.is_null() {
        unsafe {
            (*param).sched_priority = 0;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sched_setparam(
    _pid: c_int,
    _param: *const SchedParam,
) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sched_getscheduler(_pid: c_int) -> c_int {
    0 // SCHED_OTHER
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sched_setscheduler(
    _pid: c_int,
    _policy: c_int,
    _param: *const SchedParam,
) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sched_get_priority_min(_policy: c_int) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sched_get_priority_max(_policy: c_int) -> c_int {
    99
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sched_rr_get_interval(
    _pid: c_int,
    tp: *mut crate::time::TimeSpec,
) -> c_int {
    if !tp.is_null() {
        unsafe {
            (*tp).tv_sec = 0;
            (*tp).tv_nsec = 100_000_000;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setlogmask(maskpri: c_int) -> c_int {
    static MASK: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0xff);
    if maskpri == 0 {
        MASK.load(std::sync::atomic::Ordering::Relaxed)
    } else {
        MASK.swap(maskpri, std::sync::atomic::Ordering::Relaxed)
    }
}
