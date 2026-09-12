//! Initial C ABI surface for the future Linux-compatible libc.

use core::ffi::c_void;
use core::ptr;
use kinakaze_abi::ABI_ERROR;

#[cfg(all(windows, target_arch = "x86_64"))]
pub(crate) fn trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_TRACE").is_some())
}

#[cfg(all(windows, target_arch = "x86_64"))]
pub(crate) fn fork_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_FORK_TRACE").is_some())
}

#[cfg(all(windows, target_arch = "x86_64"))]
pub(crate) fn wait_trace_enabled() -> bool {
    static OWNER_PID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    let Some(target) = std::env::var_os("KINAKAZE_WAIT_TRACE") else {
        return false;
    };
    let target = target.to_string_lossy();
    let matches = target == "1"
        || std::env::args_os().any(|argument| argument.to_string_lossy().contains(target.as_ref()));
    matches && *OWNER_PID.get_or_init(std::process::id) == std::process::id()
}

#[cfg(all(windows, target_arch = "x86_64"))]
pub mod argp;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod argz;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod backtrace;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod bpf;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod catalog;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod copied;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod dirent;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod exec;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod fdio;
#[cfg(target_arch = "x86_64")]
pub mod format;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod fortify;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod fs;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod fsextra;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod fts;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod ftw;
#[cfg(all(windows, target_arch = "x86_64"))]
mod futex;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod getopt;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod glob;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod iconv;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod jump;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod locale;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod malloc_info;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod math_compat;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod memory_lock;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod misc;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod net;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod netdb;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod obstack;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod process;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod process_memory;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod pthread;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod rand48;
pub mod random_state;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod regex;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod scan;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod search;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod sigextra;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod signal;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod startup;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod stdio;
#[cfg(target_arch = "x86_64")]
pub mod strextra;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod string;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod sysadmin;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod sysdb;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod sysvipc;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod term;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod time;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod uchar;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod userdb;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod variadic;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod xattr;

pub(crate) const ENOMEM: i32 = 12;
pub(crate) const EINVAL: i32 = 22;
const ENOSYS: i32 = 38;

/// Restores the pending exec handoff after the Windows loader has finished
/// attaching this DLL. The ELF loader resolves and calls this entry explicitly
/// so the work runs outside the Windows loader lock.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "system" fn kinakaze_process_consume_exec_handoff() -> i32 {
    match kinakaze_vfs::consume_exec_handoff() {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(()) => -1,
    }
}

/// The loader may fail before entering any guest code. Publish through this
/// DLL, which owns the restored process identity, after its Rust guards unwind.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "system" fn kinakaze_process_publish_loader_exit(status: i32) {
    kinakaze_vfs::job::publish_exit(status);
    exec::complete_vfork(false);
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "system" fn kinakaze_process_publish_loader_termination(signal: i32) {
    kinakaze_vfs::job::publish_termination(signal);
    exec::complete_vfork(false);
}

#[unsafe(no_mangle)]
/// Allocates `size` bytes from the managed arena.
///
/// # Safety
///
/// The returned pointer follows the C allocation contract and must be checked
/// for null before it is dereferenced.
pub unsafe extern "C" fn kinakaze_malloc(size: usize) -> *mut c_void {
    // SAFETY: the returned block is transferred to the C caller.
    let result = unsafe { kinakaze_alloc::guest::malloc(size) };
    if result.is_null() {
        set_errno(ENOMEM);
    }
    result.cast()
}

#[unsafe(no_mangle)]
/// Releases a managed C allocation.
///
/// # Safety
///
/// `value` must be null or a live pointer returned by this library.
pub unsafe extern "C" fn kinakaze_free(value: *mut c_void) {
    // SAFETY: C callers must pass null or a pointer obtained from this library.
    unsafe { kinakaze_alloc::guest::free(value.cast()) }
}

#[unsafe(no_mangle)]
/// Allocates and zeroes `count * size` bytes.
///
/// # Safety
///
/// The returned pointer must be checked for null before it is dereferenced.
pub unsafe extern "C" fn kinakaze_calloc(count: usize, size: usize) -> *mut c_void {
    let total = match count.checked_mul(size) {
        Some(total) => total,
        None => {
            set_errno(ENOMEM);
            return ptr::null_mut();
        }
    };
    // SAFETY: allocate a new block owned by the caller.
    let result = unsafe { kinakaze_alloc::guest::malloc(total) };
    if result.is_null() {
        set_errno(ENOMEM);
        return ptr::null_mut();
    }
    // SAFETY: the allocation contains at least `total` writable bytes.
    unsafe { ptr::write_bytes(result, 0, total) };
    result.cast()
}

#[unsafe(no_mangle)]
/// Changes the size of a managed C allocation.
///
/// # Safety
///
/// `value` must be null or a live pointer returned by this library.
pub unsafe extern "C" fn kinakaze_realloc(value: *mut c_void, new_size: usize) -> *mut c_void {
    // C malloc guarantees alignment suitable for all fundamental types. The
    // current x86_64 targets use 16 bytes for that contract.
    let result = unsafe { kinakaze_alloc::guest::reallocate(value.cast(), 16, new_size) };
    if result.is_null() && new_size != 0 {
        set_errno(ENOMEM);
    }
    result.cast()
}

// Internal public-buffer allocations obey ELF interposition. The exported
// malloc/realloc/free bodies above remain the RTLD_NEXT base implementation.
pub(crate) unsafe fn c_malloc(size: usize) -> *mut c_void {
    let result = unsafe { kinakaze_alloc::c::malloc(size) };
    if result.is_null() {
        set_errno(ENOMEM);
    }
    result.cast()
}
pub(crate) unsafe fn c_realloc(value: *mut c_void, size: usize) -> *mut c_void {
    let result = unsafe { kinakaze_alloc::c::realloc(value.cast(), size) };
    if result.is_null() && size != 0 {
        set_errno(ENOMEM);
    }
    result.cast()
}
pub(crate) unsafe fn c_free(value: *mut c_void) {
    unsafe { kinakaze_alloc::c::free(value.cast()) };
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_malloc_usable_size(ptr: *mut c_void) -> usize {
    // SAFETY: this export has the same live-allocation precondition as the
    // allocator query it forwards to. Guest malloc entry points share this
    // allocator, so return the actual block capacity instead of a placeholder.
    unsafe { kinakaze_alloc::guest::usable_size(ptr.cast()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc_usable_size(ptr: *mut c_void) -> usize {
    unsafe { kinakaze_abi_malloc_usable_size(ptr) }
}

/// Reads from a descriptor in the unified libc FD table.
///
/// # Safety
///
/// `buffer` must name at least `count` writable bytes unless `count` is zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kinakaze_read(fd: i32, buffer: *mut c_void, count: usize) -> isize {
    if buffer.is_null() && count != 0 {
        set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    // SAFETY: the caller supplies a writable C buffer of `count` bytes.
    let buffer = if count == 0 {
        &mut []
    } else {
        unsafe { core::slice::from_raw_parts_mut(buffer.cast(), count) }
    };
    match kinakaze_vfs::read(fd, buffer) {
        Ok(read) => read as isize,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// Writes to a descriptor in the unified libc FD table.
///
/// # Safety
///
/// `buffer` must name at least `count` readable bytes unless `count` is zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kinakaze_write(fd: i32, buffer: *const c_void, count: usize) -> isize {
    if buffer.is_null() && count != 0 {
        set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    // SAFETY: the caller supplies a readable C buffer of `count` bytes.
    let buffer = if count == 0 {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(buffer.cast(), count) }
    };
    match kinakaze_vfs::write(fd, buffer) {
        Ok(written) => written as isize,
        Err(error) => {
            if error == 27 {
                kinakaze_vfs::signal::deliver_pending();
            }
            set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kinakaze_close(fd: i32) -> i32 {
    match kinakaze_vfs::close(fd) {
        Ok(()) => 0,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
/// Clones the current process through the runtime backend.
///
/// # Safety
///
/// The same single-threaded and live-value restrictions as
/// [`kinakaze_runtime::fork`] apply.
pub unsafe extern "C" fn kinakaze_fork() -> i32 {
    match unsafe { kinakaze_runtime::fork() } {
        Ok(pid) => pid,
        Err(error) => {
            set_errno(if error.os_code == 0 {
                ENOSYS
            } else {
                error.os_code as i32
            });
            ABI_ERROR
        }
    }
}

/// Temporary errno accessor. A real per-thread errno slot belongs to the libc
/// TLS milestone; the single-threaded fork prototype intentionally uses one
/// process-global cell.
#[unsafe(no_mangle)]
pub extern "C" fn kinakaze_errno() -> i32 {
    kinakaze_tls::errno()
}

#[unsafe(no_mangle)]
pub extern "C" fn kinakaze___errno_location() -> *mut i32 {
    kinakaze_tls::errno_location()
}

pub(crate) fn set_errno(value: i32) {
    kinakaze_tls::set_errno(value);
}

// ELF code uses the System V x86_64 calling convention even while it is hosted
// inside a Windows process. These unprefixed exports are the ABI surface loaded
// from libc.dll and written directly into ELF GOT/PLT relocations.

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_malloc(size: usize) -> *mut c_void {
    unsafe { kinakaze_malloc(size) }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_free(value: *mut c_void) {
    unsafe { kinakaze_free(value) }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_calloc(count: usize, size: usize) -> *mut c_void {
    unsafe { kinakaze_calloc(count, size) }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_realloc(value: *mut c_void, size: usize) -> *mut c_void {
    unsafe { kinakaze_realloc(value, size) }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_memalign(
    memptr: *mut *mut c_void,
    alignment: usize,
    size: usize,
) -> core::ffi::c_int {
    if memptr.is_null() {
        return EINVAL;
    }
    if alignment < core::mem::size_of::<usize>() || !alignment.is_power_of_two() {
        return EINVAL;
    }
    let p = unsafe { kinakaze_alloc::guest::memalign(alignment, size) };
    if p.is_null() {
        return ENOMEM;
    }
    unsafe { *memptr = p.cast() };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_aligned_alloc(
    alignment: usize,
    size: usize,
) -> *mut c_void {
    // GNU aligned_alloc accepts non-multiple sizes (coreutils uses an extra
    // sentinel byte). The returned allocation still covers the entire size.
    if !alignment.is_power_of_two() {
        set_errno(EINVAL);
        return ptr::null_mut();
    }
    let pointer: *mut c_void = unsafe { kinakaze_alloc::guest::memalign(alignment, size).cast() };
    if pointer.is_null() {
        set_errno(ENOMEM);
    }
    pointer
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_memalign(alignment: usize, size: usize) -> *mut c_void {
    if !alignment.is_power_of_two() {
        return ptr::null_mut();
    }
    unsafe { kinakaze_alloc::guest::memalign(alignment, size).cast() }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_read(
    fd: i32,
    buffer: *mut c_void,
    count: usize,
) -> isize {
    unsafe { kinakaze_read(fd, buffer, count) }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_write(
    fd: i32,
    buffer: *const c_void,
    count: usize,
) -> isize {
    unsafe { kinakaze_write(fd, buffer, count) }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_close(fd: i32) -> i32 {
    kinakaze_close(fd)
}

/// Process creation delegates to the canonical process coordinator.
#[cfg(all(windows, target_arch = "x86_64"))]
pub(crate) unsafe fn clone_process(parent: Option<u32>, initializer: usize, flags: u64) -> i32 {
    unsafe {
        kinakaze_runtime::kinakaze_process_clone_fork(parent.unwrap_or(0), initializer, flags)
    }
}

pub(crate) fn set_process_thread_pointer(value: usize) -> bool {
    kinakaze_tls::set_current_thread_fs_base(value).is_ok()
}

pub(crate) fn process_thread_pointer() -> usize {
    kinakaze_tls::get_current_thread_fs_base()
}

#[cfg(all(windows, target_arch = "x86_64"))]
pub(crate) fn process_clone_thread_entry() -> usize {
    kinakaze_runtime::services::engine()
        .map_or(0, |engine| engine.clone_thread as *const () as usize)
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fork() -> i32 {
    // The executable owns address-space mappings and the bootstrap entry. A
    // provider-local process clone cannot preserve that state, so the process
    // coordinator is mandatory.
    // SAFETY: initialized by the provider CRT and immutable afterward.
    let address = kinakaze_runtime::kinakaze_process_fork as *const () as usize;
    if address != 0 {
        // SAFETY: the host export has this System V returns-twice ABI.
        let host: unsafe extern "sysv64" fn() -> i32 = unsafe { core::mem::transmute(address) };
        let result = unsafe { host() };
        if fork_trace_enabled() {
            let direct_thread_pointer = kinakaze_tls::get_current_thread_fs_base();
            let process_thread_pointer = process_thread_pointer();
            let process_tls_g = if process_thread_pointer >= core::mem::size_of::<usize>() {
                unsafe {
                    ((process_thread_pointer - core::mem::size_of::<usize>()) as *const usize)
                        .read_unaligned()
                }
            } else {
                0
            };
            let direct_context = (direct_thread_pointer != 0).then(|| unsafe {
                (
                    ((direct_thread_pointer
                        + kinakaze_tls::thread_pointer::ACTIVE_GUEST_STACK_POINTER_OFFSET)
                        as *const usize)
                        .read_unaligned(),
                    ((direct_thread_pointer
                        + kinakaze_tls::thread_pointer::ACTIVE_GUEST_INSTRUCTION_POINTER_OFFSET)
                        as *const usize)
                        .read_unaligned(),
                )
            });
            eprintln!(
                "kinakaze libc: fork returned {result}, process thread pointer={process_thread_pointer:#x}, process tls g={process_tls_g:#x}, active guest context={:?}, direct thread pointer={direct_thread_pointer:#x}, direct context={direct_context:?}",
                kinakaze_tls::thread_pointer::active_guest_signal_context(),
            );
        }
        if result < 0 {
            set_errno(-result);
            return -1;
        }
        return result;
    }
    set_errno(ENOSYS);
    -1
}

/// `clone`: starts a process and enters `function(argument)` on `child_stack`.
///
/// Namespace creation is committed in the restored child before its callback.
/// Sharing an address space uses the raw clone thread entry point.
///
/// # Safety
///
/// `child_stack` must name the top of a writable stack and `function` must remain
/// callable in the cloned child.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_clone(
    function: Option<unsafe extern "sysv64" fn(*mut core::ffi::c_void) -> i32>,
    child_stack: *mut core::ffi::c_void,
    flags: i32,
    argument: *mut core::ffi::c_void,
) -> i32 {
    const CSIGNAL: u32 = 0xff;
    const SIGCHLD: u32 = 17;
    const CLONE_PARENT: u32 = 0x0000_8000;
    const SUPPORTED: u32 = CSIGNAL | CLONE_PARENT | 0x7e02_0000;

    let Some(function) = function else {
        set_errno(kinakaze_vfs::EINVAL);
        return -1;
    };
    if child_stack.is_null() {
        set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    let flags = flags as u32;
    if flags & !SUPPORTED != 0 || flags & CSIGNAL != SIGCHLD {
        set_errno(ENOSYS);
        return -1;
    }

    let parent = if flags & CLONE_PARENT != 0 {
        kinakaze_vfs::job::process_info(kinakaze_vfs::job::process_id())
            .map(|info| info.entry.ppid)
            .filter(|parent| *parent != 0)
    } else {
        None
    };
    if flags & CLONE_PARENT != 0 && parent.is_none() {
        set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }

    // SAFETY: initialized once by the provider CRT and immutable afterward.
    let address = kinakaze_runtime::kinakaze_process_clone_fork as *const () as usize;
    if address == 0 {
        set_errno(ENOSYS);
        return -1;
    }
    // SAFETY: the host export has this exact returns-twice ABI.
    let clone_fork: unsafe extern "sysv64" fn(u32, usize, u64) -> i32 =
        unsafe { core::mem::transmute(address) };
    let result = unsafe {
        clone_fork(
            parent.unwrap_or(0),
            crate::sysadmin::initialize_process_namespaces as *const () as usize,
            u64::from(flags),
        )
    };
    if result < 0 {
        set_errno(-result);
        return -1;
    }
    if result > 0 {
        return result;
    }

    // SAFETY: this is the cloned child, and the caller supplied both the stack
    // and callback according to clone's ABI.
    unsafe { enter_clone_child(function, child_stack, argument) }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(naked)]
unsafe extern "sysv64" fn enter_clone_child(
    _function: unsafe extern "sysv64" fn(*mut core::ffi::c_void) -> i32,
    _child_stack: *mut core::ffi::c_void,
    _argument: *mut core::ffi::c_void,
) -> ! {
    core::arch::naked_asm!(
        "and rsi, -16",
        "mov rsp, rsi",
        "xor ebp, ebp",
        "mov rax, rdi",
        "mov rdi, rdx",
        "call rax",
        "mov edi, eax",
        "call {exit}",
        "ud2",
        exit = sym crate::process::kinakaze_abi__exit,
    )
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___errno_location() -> *mut i32 {
    kinakaze___errno_location()
}

/// Opaque fs_struct transfer kept in the canonical libc provider across DLLs.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "system" fn kinakaze_thread_fs_capture() -> usize {
    kinakaze_vfs::fs_context::capture(true)
        .map(|value| Box::into_raw(Box::new(value)) as usize)
        .unwrap_or(0)
}
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn kinakaze_thread_fs_adopt(packet: usize) {
    if packet != 0 {
        unsafe {
            (&*(packet as *const kinakaze_vfs::fs_context::Inheritance)).adopt();
        }
    }
}
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn kinakaze_thread_fs_release(packet: usize) {
    if packet != 0 {
        drop(unsafe { Box::from_raw(packet as *mut kinakaze_vfs::fs_context::Inheritance) });
    }
}

mod object_layout;
