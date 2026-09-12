//! Process replacement, creation and reaping.
//!
//! The `exec` family, `system`, `popen`, `daemon`, and the `wait` calls that
//! collect what those produce. Three decisions here are load-bearing enough to
//! state up front, because each one is a place where Windows cannot express what
//! Linux promises and the substitution is visible to the guest.
//!
//! **`execve` spawns a fresh loader, but preserves the Linux pid.** Replacing the
//! Windows executable mapping in place is not available, so the old loader waits
//! as a native wrapper. The replacement starts suspended while the shared PID
//! namespace rebinds the original Linux identity to its real Windows pid, then it
//! is resumed. Parent waits, job control and code inside the new image therefore
//! all observe the same stable pid.
//!
//! **Descriptors without `O_CLOEXEC` survive an exec.** The replacement loader
//! reconstructs the guest descriptor table from an explicit shared-memory
//! handoff. Kernel handles are inherited only for those entries, and Winsock
//! sockets additionally cross `WSADuplicateSocketW`/`WSASocketW` so the new
//! process owns a real provider reference rather than a fragile raw handle.
//!
//! **A guest-side `/bin/sh` is what `system` and `popen` run.** Not `cmd.exe`:
//! the string being run is a POSIX shell command, and handing it to `cmd.exe`
//! would reinterpret its quoting, redirection and `$` expansion under different
//! rules. `/bin/sh` is resolved through the guest file system, so on a tree where
//! that is BusyBox it is BusyBox's shell. When no `/bin/sh` exists, `system`
//! reports the status of a shell that exited 127 — exactly what a real `system`
//! reports when it cannot execute the shell — and `popen` fails with `ENOENT`.
//!
//! Child bookkeeping lives in this module. `fork` (in [`crate`]) returns a
//! Windows pid and closes its handle, keeping no table, so this module builds
//! one: children it creates itself are recorded with their handle held open, and
//! `fork` children are discovered on demand by walking the process list for
//! entries whose parent is this process. The consequence of `fork` not holding a
//! handle is described at [`discover_children`].

#[cfg(all(windows, target_arch = "x86_64"))]
use core::arch::naked_asm;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, ResumeThread};

use crate::set_errno;

const ECHILD: c_int = 10;
const E2BIG: c_int = 7;
const EFAULT: c_int = 14;
const EINVAL: c_int = 22;
const ENOSYS: c_int = 38;
const MAX_EXEC_ITEMS: usize = 1 << 20;
const CREATE_SUSPENDED: u32 = 0x0000_0004;

type HostWait = unsafe extern "sysv64" fn(c_int, c_int, c_int, *mut c_int) -> c_int;
type HostRegisterChild = unsafe extern "sysv64" fn(u32, usize) -> c_int;
type HostVfork = unsafe extern "sysv64" fn(usize) -> c_int;
type HostVforkComplete = unsafe extern "sysv64" fn(c_int);

#[derive(Clone, Copy)]
struct ProcessEntries {
    wait: usize,
    register_child: usize,
    vfork: usize,
    vfork_complete: usize,
}

fn process_entries() -> ProcessEntries {
    ProcessEntries {
        wait: kinakaze_runtime::kinakaze_process_wait as *const () as usize,
        register_child: kinakaze_runtime::kinakaze_process_register_child as *const () as usize,
        vfork: kinakaze_runtime::kinakaze_process_vfork as *const () as usize,
        vfork_complete: kinakaze_runtime::kinakaze_process_vfork_complete as *const () as usize,
    }
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtResumeProcess(process: windows_sys::Win32::Foundation::HANDLE) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

fn trace_spawn_phase(started: &std::time::Instant, operation: &str, phase: &str) {
    if spawn_trace_enabled() {
        eprintln!(
            "kinakaze spawn: pid={} op={} phase={} elapsed_us={}",
            std::process::id(),
            operation,
            phase,
            started.elapsed().as_micros()
        );
    }
}

fn spawn_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_SPAWN_TRACE").is_some())
}

/// The hosting loader cannot change during this process image. Resolve its
/// path once instead of asking Windows to rebuild it for every exec/spawn.
fn loader_path() -> std::io::Result<&'static std::path::Path> {
    static LOADER: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    if let Some(loader) = LOADER.get() {
        return Ok(loader.as_path());
    }
    let discovered = std::env::current_exe()?;
    let _ = LOADER.set(discovered);
    Ok(LOADER
        .get()
        .expect("the loader path was initialized above")
        .as_path())
}

struct SpawnedProcess {
    process: windows_sys::Win32::Foundation::HANDLE,
    thread: windows_sys::Win32::Foundation::HANDLE,
    pid: u32,
}

impl SpawnedProcess {
    fn id(&self) -> u32 {
        self.pid
    }

    fn resume(&mut self) -> Result<(), c_int> {
        let previous = unsafe { ResumeThread(self.thread) };
        if previous == u32::MAX {
            return Err(kinakaze_vfs::errno_from_win32(unsafe { GetLastError() }));
        }
        // The primary-thread handle is no longer needed once it has resumed.
        unsafe { CloseHandle(self.thread) };
        self.thread = ptr::null_mut();
        Ok(())
    }

    fn wait(self) -> std::io::Result<i32> {
        unsafe {
            if windows_sys::Win32::System::Threading::WaitForSingleObject(self.process, u32::MAX)
                == u32::MAX
            {
                return Err(std::io::Error::last_os_error());
            }
            let mut exit_code = 0u32;
            if windows_sys::Win32::System::Threading::GetExitCodeProcess(
                self.process,
                &mut exit_code,
            ) != 0
            {
                Ok(exit_code as i32)
            } else {
                let err = GetLastError();
                Err(std::io::Error::from_raw_os_error(err as i32))
            }
        }
    }

    fn kill(&self) -> std::io::Result<()> {
        unsafe {
            if windows_sys::Win32::System::Threading::TerminateProcess(self.process, 1) != 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        }
    }
}

impl AsRawHandle for SpawnedProcess {
    fn as_raw_handle(&self) -> RawHandle {
        self.process.cast()
    }
}

impl Drop for SpawnedProcess {
    fn drop(&mut self) {
        unsafe {
            if !self.thread.is_null() {
                CloseHandle(self.thread);
            }
            if !self.process.is_null() {
                CloseHandle(self.process);
            }
        }
    }
}

fn make_command_line(args: &[String]) -> Vec<u16> {
    let mut result = String::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            result.push(' ');
        }
        if arg.is_empty() {
            result.push_str("\"\"");
        } else if !arg.contains([' ', '\t', '\n', '\x0b', '\"']) {
            result.push_str(arg);
        } else {
            result.push('\"');
            let mut backslashes = 0;
            for c in arg.chars() {
                if c == '\\' {
                    backslashes += 1;
                } else if c == '\"' {
                    result.extend(std::iter::repeat('\\').take(backslashes * 2 + 1));
                    result.push('\"');
                    backslashes = 0;
                } else {
                    result.extend(std::iter::repeat('\\').take(backslashes));
                    result.push(c);
                    backslashes = 0;
                }
            }
            result.extend(std::iter::repeat('\\').take(backslashes * 2));
            result.push('\"');
        }
    }
    result.encode_utf16().chain(std::iter::once(0)).collect()
}

fn duplicate_inheritable_handle(raw: usize) -> std::io::Result<OwnedHandle> {
    let current = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() };
    let mut duplicate = ptr::null_mut();
    let ok = unsafe {
        windows_sys::Win32::Foundation::DuplicateHandle(
            current,
            raw as windows_sys::Win32::Foundation::HANDLE,
            current,
            &mut duplicate,
            0,
            1,
            windows_sys::Win32::Foundation::DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicate.cast()) })
}

fn null_standard_handle(readable: bool) -> std::io::Result<OwnedHandle> {
    let file = if readable {
        std::fs::OpenOptions::new().read(true).open("NUL")?
    } else {
        std::fs::OpenOptions::new().write(true).open("NUL")?
    };
    let handle = OwnedHandle::from(file);
    let raw = handle.as_raw_handle().cast();
    let ok = unsafe {
        windows_sys::Win32::Foundation::SetHandleInformation(
            raw,
            windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT,
            windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(handle)
}

fn exec_standard_handle(
    fd: c_int,
    readable: bool,
) -> std::io::Result<(windows_sys::Win32::Foundation::HANDLE, Option<OwnedHandle>)> {
    match kinakaze_vfs::get(fd) {
        // Many Linux descriptors use a section or event as their lifetime
        // token. Such a token is not a Windows standard I/O handle. The guest
        // descriptor itself is transferred separately by the VFS handoff.
        Ok(entry)
            if entry.raw != 0
                && matches!(
                    entry.kind,
                    kinakaze_vfs::FdKind::File
                        | kinakaze_vfs::FdKind::Console
                        | kinakaze_vfs::FdKind::Pipe
                ) =>
        {
            // Keep the native standard handle independent of guest close and
            // FD_CLOEXEC processing, including descriptors that were borrowed.
            let duplicate = duplicate_inheritable_handle(entry.raw)?;
            let raw = duplicate.as_raw_handle().cast();
            Ok((raw, Some(duplicate)))
        }
        _ => {
            let null = null_standard_handle(readable)?;
            let raw = null.as_raw_handle().cast();
            Ok((raw, Some(null)))
        }
    }
}

fn spawn_suspended(command: &mut Command) -> std::io::Result<Child> {
    command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
    // posix_spawn is fork followed by exec: every descriptor without
    // FD_CLOEXEC must reach the new loader so its VFS handoff can reconstruct
    // the same Linux fd table. Filtering the complete table here left only
    // Win32 stdin/stdout/stderr alive, so helpers such as OpenJDK's
    // jspawnhelper received valid fd numbers in argv but EBADF for the pipes.
    kinakaze_vfs::with_execve_handle_filter(|| command.spawn())?
}

fn spawn_suspended_exec(
    loader: &std::path::Path,
    cmd_args: &[String],
    cwd: Option<&std::path::Path>,
    adoption: Option<&str>,
    descriptors: Option<&kinakaze_vfs::ExecFdSnapshot>,
) -> std::io::Result<SpawnedProcess> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW,
    };
    let app_name: Vec<u16> = loader
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut cmd_line = make_command_line(cmd_args);
    let cwd_buf = cwd.map(|p| {
        p.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    });

    let (stdin_handle, _stdin_owner) = exec_standard_handle(0, true)?;
    let (stdout_handle, _stdout_owner) = exec_standard_handle(1, false)?;
    let (stderr_handle, _stderr_owner) = exec_standard_handle(2, false)?;

    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdInput = stdin_handle;
    si.hStdOutput = stdout_handle;
    si.hStdError = stderr_handle;

    // Only this child sees the reservation: never mutate a multithreaded
    // parent's process environment around CreateProcessW.
    let environment = adoption.map(|ticket| {
        let mut entries: Vec<(std::ffi::OsString, std::ffi::OsString)> = std::env::vars_os()
            .filter(|(key, _)| {
                !key.to_string_lossy()
                    .eq_ignore_ascii_case("KINAKAZE_V2_ADOPTION")
            })
            .collect();
        entries.push(("KINAKAZE_V2_ADOPTION".into(), ticket.into()));
        entries.sort_by_key(|(key, _)| key.to_string_lossy().to_uppercase());
        let mut block = Vec::new();
        for (key, value) in entries {
            block.extend(key.encode_wide());
            block.push(b'=' as u16);
            block.extend(value.encode_wide());
            block.push(0);
        }
        block.push(0);
        block
    });
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let create = || unsafe {
        let success = CreateProcessW(
            app_name.as_ptr(),
            cmd_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,                                           // bInheritHandles = TRUE!
            CREATE_SUSPENDED | CREATE_NO_WINDOW | 0x400, // CREATE_UNICODE_ENVIRONMENT
            environment
                .as_ref()
                .map_or(std::ptr::null(), |block| block.as_ptr().cast()),
            cwd_buf.as_ref().map_or(std::ptr::null(), |b| b.as_ptr()),
            &si,
            &mut pi,
        );
        // Capture last-error before restoring other handles can change it.
        if success == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    };
    match descriptors {
        Some(snapshot) => kinakaze_vfs::with_checked_exec_handle_filter(snapshot, create),
        None => kinakaze_vfs::with_execve_handle_filter(create),
    }??;

    Ok(SpawnedProcess {
        process: pi.hProcess,
        thread: pi.hThread,
        pid: pi.dwProcessId,
    })
}

/// Resumes a process created with `CREATE_SUSPENDED` through the process handle
/// already owned by `Child`. This avoids taking a system-wide thread snapshot
/// merely to rediscover the primary thread that `CreateProcessW` just created.
fn resume_suspended_process(child: &Child) -> Result<(), c_int> {
    // SAFETY: `Child` owns a live process handle with the access granted by
    // CreateProcessW, and this call does not take ownership of that handle.
    let status = unsafe { NtResumeProcess(child.as_raw_handle().cast()) };
    if status >= 0 {
        return Ok(());
    }

    // SAFETY: `status` is the failing NTSTATUS returned immediately above.
    let win32_error = unsafe { RtlNtStatusToDosError(status) };
    Err(kinakaze_vfs::errno_from_win32(win32_error))
}

/// Finds the process-wide wait coordinator exported by the hosting ELF loader.
///
/// The coordinator owns the real Windows process handles.  Keeping that table
/// in libc would split it between the executable and every normally loaded
/// substitute DLL, which is exactly the kind of PE-module state fork must not
/// try to reproduce by copying `.data`.
fn host_wait() -> Option<HostWait> {
    let address = process_entries().wait;
    if address == 0 {
        return None;
    }
    // SAFETY: the provider initializer resolved the coordinator's typed entry.
    Some(unsafe { core::mem::transmute(address) })
}

fn register_waitable_child(pid: u32, child: &impl AsRawHandle) -> bool {
    let address = process_entries().register_child;
    if address == 0 {
        return false;
    }
    // SAFETY: the executable exports this exact coordinator signature.
    let register: HostRegisterChild = unsafe { core::mem::transmute(address) };
    unsafe { register(pid, child.as_raw_handle() as usize) != 0 }
}

const WNOHANG: c_int = 1;
const WUNTRACED: c_int = 2;
const WCONTINUED: c_int = 8;
const WNOWAIT: c_int = 0x0100_0000;

const WAIT_EVENT_EXITED: c_int = 1;
const WAIT_EVENT_STOPPED: c_int = 2;
const WAIT_EVENT_CONTINUED: c_int = 4;

unsafe fn wait_host(pid: c_int, options: c_int, events: c_int, status: *mut c_int) -> c_int {
    let Some(wait) = host_wait() else {
        set_errno(ENOSYS);
        return -1;
    };
    let result = unsafe { wait(pid, options, events, status) };
    if result < 0 {
        set_errno(result.checked_neg().unwrap_or(ECHILD));
        -1
    } else {
        result
    }
}

/// Linux `waitpid(2)` over children created by the runtime fork coordinator.
///
/// Every selector is supported: one pid, any child (`-1`), any child in the
/// caller's own process group (`0`), and any child in a named group (`< -1`).
/// `WNOHANG`, `WUNTRACED` and `WCONTINUED` are all honoured, and the status the
/// coordinator writes distinguishes an exit from a fatal signal, a stop and a
/// continue — so `WIFSIGNALED`, `WIFSTOPPED` and `WIFCONTINUED` mean what the
/// guest expects rather than always reading as an ordinary exit.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_waitpid(
    pid: c_int,
    status: *mut c_int,
    options: c_int,
) -> c_int {
    if options & !(WNOHANG | WUNTRACED | WCONTINUED) != 0 {
        set_errno(EINVAL);
        return -1;
    }
    if crate::fork_trace_enabled() || crate::wait_trace_enabled() {
        eprintln!(
            "kinakaze libc: pid {} waitpid(pid={}, options={:#x}) starting",
            std::process::id(),
            pid,
            options
        );
    }
    let mut events = WAIT_EVENT_EXITED;
    if options & WUNTRACED != 0 {
        events |= WAIT_EVENT_STOPPED;
    }
    if options & WCONTINUED != 0 {
        events |= WAIT_EVENT_CONTINUED;
    }
    // SAFETY: the optional status pointer is passed through unchanged.
    let result = unsafe { wait_host(pid, options & WNOHANG, events, status) };
    let status_val = if !status.is_null() && result > 0 {
        unsafe { *status }
    } else {
        -1
    };
    if crate::fork_trace_enabled() || crate::wait_trace_enabled() {
        eprintln!(
            "kinakaze libc: pid {} waitpid(pid={}, options={:#x}) returned {} (status={:#x})",
            std::process::id(),
            pid,
            options,
            result,
            status_val
        );
    }
    result
}

/// Linux `wait(2)`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wait(status: *mut c_int) -> c_int {
    // SAFETY: this is exactly waitpid(-1, status, 0).
    unsafe { kinakaze_abi_waitpid(-1, status, 0) }
}

/// Linux x86_64 `wait4(2)`. Resource accounting is currently unavailable, but
/// a successfully reaped child reports a zeroed `struct rusage`, rather than
/// leaking uninitialised host bytes to the guest.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wait4(
    pid: c_int,
    status: *mut c_int,
    options: c_int,
    usage: *mut c_void,
) -> c_int {
    // SAFETY: the arguments have waitpid's contract.
    let result = unsafe { kinakaze_abi_waitpid(pid, status, options) };
    if result > 0 && !usage.is_null() {
        // Linux x86_64 struct rusage: two 16-byte timevals and fourteen longs.
        // SAFETY: wait4 requires a writable struct rusage when this is non-null.
        unsafe { ptr::write_bytes(usage.cast::<u8>(), 0, 144) };
    }
    result
}

/// Linux `wait3(2)`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wait3(
    status: *mut c_int,
    options: c_int,
    usage: *mut c_void,
) -> c_int {
    // SAFETY: wait3 is wait4(-1, ...).
    unsafe { kinakaze_abi_wait4(-1, status, options, usage) }
}

/// Reads a null-terminated C vector without assigning ownership to its strings.
unsafe fn string_vector(mut vector: *const *const c_char) -> Result<Vec<String>, c_int> {
    if vector.is_null() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for _ in 0..MAX_EXEC_ITEMS {
        let value = unsafe { *vector };
        if value.is_null() {
            return Ok(result);
        }
        let bytes = unsafe { CStr::from_ptr(value) }.to_bytes();
        result.push(String::from_utf8_lossy(bytes).into_owned());
        vector = unsafe { vector.add(1) };
    }
    Err(E2BIG)
}

fn exec_host_path(path: &str) -> Result<std::path::PathBuf, c_int> {
    kinakaze_vfs::mount::overlay::check_execute(path)?;
    if crate::trace_enabled() {
        eprintln!("kinakaze: [TRACE exec_host_path] path={path:?}");
    }
    if path == "/proc/self/exe" {
        // Linux exposes the current guest image as a procfs symlink. Resolve
        // that guest path through the VFS first so fork children do not depend
        // on a loader-private Windows environment variable being reconstructed.
        if let Ok(meta) = kinakaze_vfs::procfs::metadata(path) {
            if let Some(target) = meta.target {
                kinakaze_vfs::mount::overlay::check_execute(&target)?;
                if let Ok(resolved) = kinakaze_vfs::resolve_linux_path(&target) {
                    if resolved.is_file() {
                        return Ok(resolved);
                    }
                }
            }
        }
        return Err(kinakaze_vfs::ENOENT);
    }
    if let Some(fd_str) = path
        .strip_prefix("/proc/self/fd/")
        .or_else(|| path.strip_prefix("/dev/fd/"))
    {
        let fd_clean = fd_str.trim_end_matches('\0');
        if let Ok(fd) = fd_clean.parse::<i32>() {
            let proc_target = format!("/proc/self/fd/{fd}");
            if let Ok(meta) = kinakaze_vfs::procfs::metadata(&proc_target) {
                if let Some(target) = meta.target {
                    if let Ok(resolved) = kinakaze_vfs::resolve_linux_path(&target) {
                        if resolved.is_file() {
                            return Ok(resolved);
                        }
                    }
                }
            }
        }
    }
    let absolute = if path.starts_with('/') {
        path.to_owned()
    } else {
        let cwd = kinakaze_vfs::fs::getcwd();
        if cwd == "/" {
            format!("/{path}")
        } else {
            format!("{cwd}/{path}")
        }
    };
    if let Ok(resolved) = kinakaze_vfs::resolve_linux_path(&absolute) {
        if resolved.is_file() {
            return Ok(resolved);
        }
    }
    Err(kinakaze_vfs::ENOENT)
}

fn command_stdio(fd: c_int) -> Result<Stdio, c_int> {
    let entry = kinakaze_vfs::get(fd)?;
    if entry.raw == 0 {
        return Err(kinakaze_vfs::EBADF);
    }
    unsafe {
        let current_process = windows_sys::Win32::System::Threading::GetCurrentProcess();
        let mut target_handle: windows_sys::Win32::Foundation::HANDLE = ptr::null_mut();
        let ok = windows_sys::Win32::Foundation::DuplicateHandle(
            current_process,
            entry.raw as windows_sys::Win32::Foundation::HANDLE,
            current_process,
            &mut target_handle,
            0,
            1, // bInheritHandle = TRUE so CreateProcess inherits standard stdio handles
            windows_sys::Win32::Foundation::DUPLICATE_SAME_ACCESS,
        );
        if ok == 0 {
            return Err(kinakaze_vfs::EBADF);
        }
        let owned = std::os::windows::io::OwnedHandle::from_raw_handle(target_handle as RawHandle);
        Ok(Stdio::from(owned))
    }
}

/// Replaces the guest image while preserving the manager's logical process.
///
/// The fresh loader receives the exact guest argv/envp and duplicates the
/// current guest descriptors 0, 1 and 2 as its Windows standard handles. The
/// manager activates the replacement only after the original native worker and
/// all its threads have died. The same Linux PID and inherited state continue
/// in the new image; precommit failures preserve the calling image.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_execve(
    path: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    let spawn_started = std::time::Instant::now();
    if path.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
    let guest_path = String::from_utf8_lossy(bytes).into_owned();
    trace_spawn_phase(&spawn_started, "execve", "arguments-decoded");
    if crate::trace_enabled() {
        eprintln!("kinakaze: [TRACE execve] guest_path={guest_path:?}");
    }

    let arguments = match unsafe { string_vector(argv) } {
        Ok(arguments) => arguments,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    if spawn_trace_enabled() {
        eprintln!(
            "kinakaze spawn: pid={} op=execve guest_path={guest_path:?} argv={arguments:?}",
            std::process::id(),
        );
    }
    let environment = match unsafe { string_vector(envp) } {
        Ok(environment) => environment,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    if std::env::var_os("KINAKAZE_ENV_TRACE").is_some() {
        let keys = environment
            .iter()
            .map(|entry| {
                entry.split_once('=').map_or_else(
                    || format!("{}:-", entry),
                    |(key, value)| format!("{key}:{}", value.len()),
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        eprintln!(
            "kinakaze: [TRACE execve] envp={envp:p} count={} keys={keys}",
            environment.len()
        );
    }
    let host_path = match exec_host_path(&guest_path) {
        Ok(path) if path.is_file() => path,
        _ => {
            set_errno(kinakaze_vfs::ENOENT);
            return -1;
        }
    };
    let original_path = host_path.clone();
    let original_image = match kinakaze_vfs::read_guest_image(&host_path) {
        Ok(image) => image,
        Err(error) => {
            set_errno(kinakaze_vfs::errno_from_win32(
                error.raw_os_error().unwrap_or(1) as u32,
            ));
            return -1;
        }
    };

    // Handle shebang scripts (e.g. #!/bin/sh, #!/usr/bin/env, etc.)
    let (host_path, arguments) = if original_image.starts_with(b"#!") {
        if let Some(newline_pos) = original_image
            .iter()
            .position(|&b| b == b'\n' || b == b'\r')
        {
            let shebang_line = String::from_utf8_lossy(&original_image[2..newline_pos]);
            let parts: Vec<&str> = shebang_line.split_whitespace().collect();
            if let Some(interp) = parts.first() {
                let interp_host = match exec_host_path(interp) {
                    Ok(p) if p.is_file() => p,
                    _ => host_path.clone(),
                };
                let mut new_args = Vec::new();
                new_args.push(interp.to_string());
                for extra in &parts[1..] {
                    new_args.push(extra.to_string());
                }
                // execve's pathname remains meaningful inside the caller's
                // mount/chroot namespace. A lower overlay layer's native path
                // cannot be reopened there, and argv[0] is caller-controlled.
                new_args.push(guest_path.clone());
                if arguments.len() > 1 {
                    new_args.extend(arguments[1..].iter().cloned());
                }
                (interp_host, new_args)
            } else {
                (host_path, arguments)
            }
        } else {
            (host_path, arguments)
        }
    } else {
        (host_path, arguments)
    };
    let exec_image = if host_path == original_path {
        original_image
    } else {
        match kinakaze_vfs::read_guest_image(&host_path) {
            Ok(image) => image,
            Err(error) => {
                set_errno(kinakaze_vfs::errno_from_win32(
                    error.raw_os_error().unwrap_or(1) as u32,
                ));
                return -1;
            }
        }
    };
    trace_spawn_phase(&spawn_started, "execve", "image-read");
    // Reject malformed or incompatible images while the calling image and its
    // descriptor table are still intact, before creating a native candidate.
    if kinakaze_elf::ElfFile::parse(&exec_image).is_err() {
        set_errno(8); // ENOEXEC
        return -1;
    }

    // An exec on another guest thread, or a native fork, shares these process
    // handoff participants. Only one may snapshot/rebind the image at a time.
    let _image_transaction = kinakaze_runtime::lock_process_image();

    let loader = match loader_path() {
        Ok(loader) => loader,
        Err(error) => {
            set_errno(kinakaze_vfs::errno_from_win32(
                error.raw_os_error().unwrap_or(1) as u32,
            ));
            return -1;
        }
    };
    let guest_cwd = kinakaze_vfs::fs::getcwd();
    let host_cwd = kinakaze_vfs::resolve_linux_path(&guest_cwd)
        .ok()
        .filter(|p| p.is_dir());

    let mut cmd_args = vec![
        loader.display().to_string(),
        "--kinakaze-exec".to_string(),
        host_path.to_string_lossy().into_owned(),
    ];
    if arguments.is_empty() {
        cmd_args.push(guest_path.clone());
    } else {
        cmd_args.extend(arguments.clone());
    }

    if crate::fork_trace_enabled() {
        eprintln!(
            "kinakaze libc: pid {} execve({guest_path:?}) -> {host_path:?}, argv={arguments:?}",
            std::process::id()
        );
    }
    // A recoverable exec error leaves this guest image running. In particular,
    // a vfork child may retry exec or do cleanup before `_exit`; only committed
    // replacement or actual exit may release its suspended parent.
    let exec_payload = match kinakaze_vfs::prepare_exec_state_from_image(&environment, &exec_image)
    {
        Ok(payload) => payload,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    trace_spawn_phase(&spawn_started, "execve", "handoff-serialized");
    match spawn_suspended_exec(&loader, &cmd_args, host_cwd.as_deref(), None, None) {
        Ok(mut child) => {
            trace_spawn_phase(&spawn_started, "execve", "process-created-suspended");
            let Some(handoff) = kinakaze_vfs::stage_exec_handoff(child.id(), &exec_payload) else {
                let _ = child.kill();
                kinakaze_vfs::finish_exec_state(None);
                let _ = child.wait();
                set_errno(kinakaze_vfs::EIO);
                return -1;
            };
            trace_spawn_phase(&spawn_started, "execve", "handoff-staged");
            // Reserve the original Linux identity for the candidate. Guest code
            // remains gated until the manager confirms old native process death.
            let guest_executable = kinakaze_vfs::to_guest_path(&host_path);
            let comm_source = arguments
                .first()
                .map(String::as_str)
                .unwrap_or(guest_executable.as_str());
            let comm = comm_source
                .rsplit(['/', '\\'])
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or("kinakaze");
            let namespace_pid = kinakaze_vfs::job::process_id();
            let adopted = kinakaze_vfs::job::adopt_exec_replacement(
                child.id(),
                comm,
                &guest_executable,
                &arguments,
            );
            if !adopted {
                let _ = child.kill();
                kinakaze_vfs::finish_exec_state(None);
                let _ = child.wait();
                set_errno(kinakaze_vfs::EIO);
                return -1;
            }
            if let Err(error) = child.resume() {
                let _ = child.kill();
                kinakaze_vfs::finish_exec_state(None);
                let _ = child.wait();
                if adopted {
                    kinakaze_vfs::job::rollback_exec_replacement(namespace_pid);
                }
                set_errno(error);
                return -1;
            }
            trace_spawn_phase(&spawn_started, "execve", "process-resumed");
            if !handoff.wait_until_owned(&child) {
                let _ = child.kill();
                kinakaze_vfs::finish_exec_state(None);
                let _ = child.wait();
                if adopted {
                    kinakaze_vfs::job::rollback_exec_replacement(namespace_pid);
                }
                set_errno(kinakaze_vfs::EIO);
                return -1;
            }
            trace_spawn_phase(&spawn_started, "execve", "handoff-owned");
            // The child owns the shared frame, so the parent can now publish
            // process-specific WSADuplicateSocketW recipes. This call waits for
            // libc's acknowledgement before the wrapper closes source sockets.
            kinakaze_vfs::finish_exec_state(Some(child.id()));
            trace_spawn_phase(&spawn_started, "execve", "socket-handoff-finished");
            if let Err(error) = kinakaze_vfs::job::finish_exec_replacement() {
                let _ = child.kill();
                let _ = child.wait();
                kinakaze_vfs::job::rollback_exec_replacement(namespace_pid);
                set_errno(error);
                return -1;
            }
            trace_spawn_phase(&spawn_started, "execve", "process-identity-released");
            // Release the retiring image's descriptor references before native
            // termination, including explicit shared VFS reference accounting.
            kinakaze_vfs::close_exec_wrapper_descriptors();
            trace_spawn_phase(&spawn_started, "execve", "wrapper-descriptors-closed");
            // POSIX vfork releases the parent as soon as exec has replaced the
            // child image, not when that replacement eventually exits.
            complete_vfork(true);
            trace_spawn_phase(&spawn_started, "execve", "vfork-completed");
            if kinakaze_runtime::authority::get().is_some() {
                // This is the uniform stop of every old guest/native thread.
                // The manager observes the pinned handle death before allowing
                // even an IFUNC resolver to execute in the replacement worker.
                crate::process::terminate_host_process(0);
            }
            let code = match child.wait() {
                Ok(code) => code as u32,
                Err(error) => {
                    set_errno(kinakaze_vfs::errno_from_win32(
                        error.raw_os_error().unwrap_or(1) as u32,
                    ));
                    return -1;
                }
            };
            if spawn_trace_enabled() {
                eprintln!(
                    "kinakaze spawn: pid={} op=execve replacement_exit_code={code}",
                    std::process::id(),
                );
            }
            trace_spawn_phase(&spawn_started, "execve", "replacement-exited");
            // execve does not run the old image's atexit handlers or destructors.
            crate::process::terminate_host_process(code)
        }
        Err(error) => {
            kinakaze_vfs::finish_exec_state(None);
            set_errno(kinakaze_vfs::errno_from_win32(
                error.raw_os_error().unwrap_or(1) as u32,
            ));
            -1
        }
    }
}

unsafe extern "sysv64" fn vfork_with_stack(stack_boundary: usize) -> c_int {
    let address = process_entries().vfork;
    if address != 0 {
        // SAFETY: the host publishes this exact System V ABI.
        let host: HostVfork = unsafe { core::mem::transmute(address) };
        let result = unsafe { host(stack_boundary) };
        if result < 0 {
            set_errno(-result);
            return -1;
        }
        return result;
    }
    crate::kinakaze_abi_fork()
}

/// Notifies the host when an active vfork child reaches exec or `_exit`.
pub(crate) fn complete_vfork(exec_succeeded: bool) {
    let address = process_entries().vfork_complete;
    if address == 0 {
        return;
    }
    // SAFETY: the export has the declared ABI and treats nonzero as success.
    let complete: HostVforkComplete = unsafe { core::mem::transmute(address) };
    unsafe { complete(i32::from(exec_succeeded)) };
}

/// Linux `vfork(2)`.
///
/// Capturing the entry RSP in a naked shim identifies the provider's private
/// ABI-transition stack. The process coordinator must keep that host stack out
/// of Linux CLONE_VM state rather than copying child return frames over it.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_vfork() -> c_int {
    naked_asm!(
        "mov rdi, rsp",
        "sub rsp, 8",
        "call {inner}",
        "add rsp, 8",
        "ret",
        inner = sym vfork_with_stack,
    )
}

/// Linux `execv(3)`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_execv(
    path: *const c_char,
    argv: *const *const c_char,
) -> c_int {
    let env = crate::process::kinakaze_abi_environ.get();
    unsafe { kinakaze_abi_execve(path, argv, env.cast()) }
}

/// Linux `execvpe(3)`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_execvpe(
    file: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    if file.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    let file_str = match unsafe { CStr::from_ptr(file) }.to_str() {
        Ok(s) => s,
        Err(_) => {
            set_errno(EINVAL);
            return -1;
        }
    };
    if file_str.contains('/') {
        return unsafe { kinakaze_abi_execve(file, argv, envp) };
    }

    let path_val = unsafe {
        let path_name = b"PATH\0".as_ptr().cast();
        let ptr = crate::process::kinakaze_abi_getenv(path_name);
        if ptr.is_null() {
            "/bin:/usr/bin"
        } else {
            CStr::from_ptr(ptr).to_str().unwrap_or("/bin:/usr/bin")
        }
    };

    let mut search_dirs: Vec<&str> = Vec::new();
    if path_val.contains(';') {
        search_dirs.extend(path_val.split(';').filter(|s| !s.is_empty()));
    } else {
        search_dirs.extend(path_val.split(':').filter(|s| !s.is_empty()));
    }
    for std_dir in [
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/local/sbin",
        "/usr/sbin",
        "/sbin",
    ] {
        if !search_dirs.contains(&std_dir) {
            search_dirs.push(std_dir);
        }
    }

    let mut last_error = kinakaze_vfs::ENOENT;
    for dir in search_dirs {
        let dir = if dir.is_empty() { "." } else { dir };
        let candidate = if dir.ends_with('/') || dir.ends_with('\\') {
            format!("{dir}{file_str}")
        } else {
            format!("{dir}/{file_str}")
        };
        let host_res = exec_host_path(&candidate);
        if host_res.is_err() {
            continue;
        }
        let Ok(c_candidate) = std::ffi::CString::new(candidate) else {
            continue;
        };
        let res = unsafe { kinakaze_abi_execve(c_candidate.as_ptr(), argv, envp) };
        if res != -1 {
            return res;
        }
        let err = kinakaze_tls::errno();
        if err != kinakaze_vfs::ENOENT {
            last_error = err;
        }
    }
    set_errno(last_error);
    -1
}

/// Linux `execvp(3)`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_execvp(
    file: *const c_char,
    argv: *const *const c_char,
) -> c_int {
    let env = crate::process::kinakaze_abi_environ.get();
    unsafe { kinakaze_abi_execvpe(file, argv, env.cast()) }
}

unsafe fn collect_va_argv(va: &mut crate::format::VaList) -> Vec<*const c_char> {
    let mut args = Vec::new();
    loop {
        let ptr: *const c_char = unsafe { va.next_integer() };
        args.push(ptr);
        if ptr.is_null() {
            break;
        }
    }
    args
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_execl_impl(
    path: *const c_char,
    va: *mut crate::format::VaList,
) -> c_int {
    if va.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    let argv = unsafe { collect_va_argv(&mut *va) };
    unsafe { kinakaze_abi_execv(path, argv.as_ptr()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_execlp_impl(
    file: *const c_char,
    va: *mut crate::format::VaList,
) -> c_int {
    if va.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    let argv = unsafe { collect_va_argv(&mut *va) };
    unsafe { kinakaze_abi_execvp(file, argv.as_ptr()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_execle_impl(
    path: *const c_char,
    va: *mut crate::format::VaList,
) -> c_int {
    if va.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    let argv = unsafe { collect_va_argv(&mut *va) };
    let envp: *const *const c_char = unsafe { (*va).next_integer() };
    unsafe { kinakaze_abi_execve(path, argv.as_ptr(), envp) }
}

/// Linux `system(3)`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_system(command: *const c_char) -> c_int {
    if command.is_null() {
        return 1;
    }
    let pid = crate::kinakaze_abi_fork();
    if pid < 0 {
        return -1;
    }
    if pid == 0 {
        let sh = c"/bin/sh".as_ptr();
        let flag = c"-c".as_ptr();
        let argv = [sh, flag, command, ptr::null()];
        let env = crate::process::kinakaze_abi_environ.get();
        unsafe { kinakaze_abi_execve(sh, argv.as_ptr(), env.cast()) };
        crate::process::kinakaze_abi__exit(127);
    }
    let mut status = 0;
    let waited = unsafe { kinakaze_abi_waitpid(pid, &mut status, 0) };
    if waited < 0 { -1 } else { status }
}

use std::sync::Mutex;
static POPEN_PIDS: std::sync::OnceLock<Mutex<Vec<(usize, c_int)>>> = std::sync::OnceLock::new();

fn popen_pids() -> &'static Mutex<Vec<(usize, c_int)>> {
    POPEN_PIDS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Linux `popen(3)`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_popen(
    command: *const c_char,
    mode: *const c_char,
) -> *mut crate::stdio::File {
    if command.is_null() || mode.is_null() {
        set_errno(EINVAL);
        return ptr::null_mut();
    }
    let mode_str = match unsafe { CStr::from_ptr(mode) }.to_str() {
        Ok(s) => s,
        Err(_) => {
            set_errno(EINVAL);
            return ptr::null_mut();
        }
    };
    let is_read = if mode_str.starts_with('r') {
        true
    } else if mode_str.starts_with('w') {
        false
    } else {
        set_errno(EINVAL);
        return ptr::null_mut();
    };

    let mut pipe_fds = [0i32; 2];
    if unsafe { crate::fdio::kinakaze_abi_pipe(pipe_fds.as_mut_ptr()) } < 0 {
        return ptr::null_mut();
    }
    let (read_fd, write_fd) = (pipe_fds[0], pipe_fds[1]);

    let pid = crate::kinakaze_abi_fork();
    if pid < 0 {
        crate::kinakaze_abi_close(read_fd);
        crate::kinakaze_abi_close(write_fd);
        return ptr::null_mut();
    }

    if pid == 0 {
        // Child
        if is_read {
            crate::kinakaze_abi_close(read_fd);
            crate::fdio::kinakaze_abi_dup2(write_fd, 1);
            if write_fd != 1 {
                crate::kinakaze_abi_close(write_fd);
            }
        } else {
            crate::kinakaze_abi_close(write_fd);
            crate::fdio::kinakaze_abi_dup2(read_fd, 0);
            if read_fd != 0 {
                crate::kinakaze_abi_close(read_fd);
            }
        }
        let sh = c"/bin/sh".as_ptr();
        let flag = c"-c".as_ptr();
        let argv = [sh, flag, command, ptr::null()];
        let env = crate::process::kinakaze_abi_environ.get();
        unsafe { kinakaze_abi_execve(sh, argv.as_ptr(), env.cast()) };
        crate::process::kinakaze_abi__exit(127);
    }

    // Parent
    let (parent_fd, child_close_fd) = if is_read {
        (read_fd, write_fd)
    } else {
        (write_fd, read_fd)
    };
    crate::kinakaze_abi_close(child_close_fd);

    let file = unsafe { crate::stdio::fdopen(parent_fd, mode) };
    if file.is_null() {
        crate::kinakaze_abi_close(parent_fd);
        return ptr::null_mut();
    }

    if let Ok(mut pids) = popen_pids().lock() {
        pids.push((file as usize, pid));
    }
    file
}

/// Linux `pclose(3)`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pclose(stream: *mut crate::stdio::File) -> c_int {
    if stream.is_null() {
        set_errno(EINVAL);
        return -1;
    }
    let pid = {
        let mut pids = popen_pids().lock().map_err(|_| EINVAL);
        match pids {
            Ok(ref mut table) => {
                if let Some(pos) = table.iter().position(|(f, _)| *f == stream as usize) {
                    table.remove(pos).1
                } else {
                    set_errno(ECHILD);
                    return -1;
                }
            }
            Err(err) => {
                set_errno(err);
                return -1;
            }
        }
    };

    unsafe { crate::stdio::fclose(stream) };

    let mut status = 0;
    let waited = unsafe { kinakaze_abi_waitpid(pid, &mut status, 0) };
    if waited < 0 { -1 } else { status }
}

/// Linux `daemon(3)`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_daemon(nochdir: c_int, noclose: c_int) -> c_int {
    let pid = crate::kinakaze_abi_fork();
    if pid < 0 {
        return -1;
    }
    if pid > 0 {
        crate::process::kinakaze_abi__exit(0);
    }
    let _ = crate::userdb::kinakaze_abi_setsid();
    if nochdir == 0 {
        let root = c"/".as_ptr();
        let _ = unsafe { crate::fs::exports::kinakaze_abi_chdir(root) };
    }
    if noclose == 0 {
        let devnull = c"/dev/null".as_ptr();
        let fd = unsafe { crate::fsextra::kinakaze_abi_open64(devnull, 2, 0) };
        if fd >= 0 {
            let _ = crate::fdio::kinakaze_abi_dup2(fd, 0);
            let _ = crate::fdio::kinakaze_abi_dup2(fd, 1);
            let _ = crate::fdio::kinakaze_abi_dup2(fd, 2);
            if fd > 2 {
                crate::kinakaze_abi_close(fd);
            }
        }
    }
    0
}

mod spawn;

#[repr(C)]
pub struct SigInfo {
    pub si_signo: c_int,
    pub si_errno: c_int,
    pub si_code: c_int,
    pub _pad: [u8; 116],
}

pub const P_ALL: c_int = 0;
pub const P_PID: c_int = 1;
pub const P_PGID: c_int = 2;

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_waitid(
    idtype: c_int,
    id: c_int,
    infop: *mut SigInfo,
    options: c_int,
) -> c_int {
    const WEXITED: c_int = 4;
    const WAITID_OPTIONS: c_int = WNOHANG | WUNTRACED | WEXITED | WCONTINUED | WNOWAIT;
    if options & !WAITID_OPTIONS != 0 || options & (WUNTRACED | WEXITED | WCONTINUED) == 0 {
        set_errno(EINVAL);
        return -1;
    }
    let target_pid = match idtype {
        P_ALL => -1,
        P_PID if id > 0 => id,
        P_PGID if id >= 0 => -id,
        _ => {
            set_errno(EINVAL);
            return -1;
        }
    };
    let mut status = 0;
    let mut events = 0;
    if options & WEXITED != 0 {
        events |= WAIT_EVENT_EXITED;
    }
    if options & WUNTRACED != 0 {
        events |= WAIT_EVENT_STOPPED;
    }
    if options & WCONTINUED != 0 {
        events |= WAIT_EVENT_CONTINUED;
    }
    let reaped = unsafe {
        wait_host(
            target_pid,
            options & (WNOHANG | WNOWAIT),
            events,
            &mut status,
        )
    };
    if reaped < 0 {
        return -1;
    }
    if !infop.is_null() {
        unsafe {
            ptr::write_bytes(infop.cast::<u8>(), 0, core::mem::size_of::<SigInfo>());
            if reaped == 0 {
                return 0;
            }
            (*infop).si_signo = 17; // SIGCHLD
            let pid_ptr = (infop as *mut u8).add(16) as *mut c_int;
            let status_ptr = (infop as *mut u8).add(24) as *mut c_int;
            *pid_ptr = reaped;
            if status == 0xffff {
                (*infop).si_code = 6; // CLD_CONTINUED
                *status_ptr = 18; // SIGCONT
            } else if status & 0xff == 0x7f {
                (*infop).si_code = 5; // CLD_STOPPED
                *status_ptr = (status >> 8) & 0xff;
            } else if status & 0x7f == 0 {
                (*infop).si_code = 1; // CLD_EXITED
                *status_ptr = (status >> 8) & 0xff;
            } else {
                (*infop).si_code = if status & 0x80 != 0 { 3 } else { 2 };
                *status_ptr = status & 0x7f;
            }
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fexecve(
    fd: c_int,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    let proc_path = format!("/proc/self/fd/{fd}\0");
    unsafe { kinakaze_abi_execve(proc_path.as_ptr() as *const c_char, argv, envp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn execveat(
    dirfd: c_int,
    pathname: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
    flags: c_int,
) -> c_int {
    unsafe { kinakaze_abi_execveat(dirfd, pathname, argv, envp, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_execveat(
    dirfd: c_int,
    pathname: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
    flags: c_int,
) -> c_int {
    const AT_EMPTY_PATH: c_int = 0x1000;
    const AT_FDCWD: c_int = -100;

    if pathname.is_null() {
        if flags & AT_EMPTY_PATH != 0 {
            return unsafe { kinakaze_abi_fexecve(dirfd, argv, envp) };
        }
        set_errno(EFAULT);
        return -1;
    }

    let c_str = unsafe { CStr::from_ptr(pathname) };
    let path_bytes = c_str.to_bytes();
    if path_bytes.is_empty() && (flags & AT_EMPTY_PATH != 0) {
        return unsafe { kinakaze_abi_fexecve(dirfd, argv, envp) };
    }

    let path_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => {
            set_errno(EINVAL);
            return -1;
        }
    };

    if path_str.starts_with('/') || dirfd == AT_FDCWD {
        unsafe { kinakaze_abi_execve(pathname, argv, envp) }
    } else {
        let proc_path = format!("/proc/self/fd/{dirfd}/{path_str}\0");
        unsafe { kinakaze_abi_execve(proc_path.as_ptr() as *const c_char, argv, envp) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waitpid_exit_status_bitmasks_and_macros() {
        // Normal exit with status code 42: (42 << 8) | 0
        let normal_status = 42 << 8;
        assert_eq!(normal_status & 0x7f, 0, "WIFEXITED");
        assert_eq!((normal_status >> 8) & 0xff, 42, "WEXITSTATUS");

        // Terminated by signal 9 (SIGKILL): 9 & 0x7f
        let killed_status = 9;
        assert_ne!(killed_status & 0x7f, 0, "WIFSIGNALED");
        assert_eq!(killed_status & 0x7f, 9, "WTERMSIG");

        // Stopped by signal 19 (SIGSTOP): 0x7f | (19 << 8)
        let stopped_status = 0x7f | (19 << 8);
        assert_eq!(stopped_status & 0xff, 0x7f, "WIFSTOPPED");
        assert_eq!((stopped_status >> 8) & 0xff, 19, "WSTOPSIG");
    }

    #[test]
    fn waitid_idtype_validation() {
        // Invalid idtype returns EINVAL (-1)
        let mut info: SigInfo = unsafe { core::mem::zeroed() };
        assert_eq!(unsafe { kinakaze_abi_waitid(999, 0, &mut info, 0) }, -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EINVAL);
    }
}
