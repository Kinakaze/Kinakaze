//! Linux ELF execution inside one manager-adopted V2 worker.
//! The owner links this crate once into the runtime DLL and supplies its provider registry.

pub use kinakaze_alloc::ManagedAllocator;
pub use kinakaze_link::{ProviderImage, ProviderRegistry, ProviderSymbol, ProviderSymbolKind};

mod execution;
mod fault_action;
pub mod helpers;

fn report_loader_error(message: std::fmt::Arguments<'_>) {
    if let Some(directory) = std::env::var_os("KINAKAZE_NAMESPACE_TRACE_DIR") {
        use std::io::Write;
        let path = std::path::PathBuf::from(directory)
            .join(format!("loader-errors-{}.log", std::process::id()));
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{message}");
        }
    }
    eprintln!("{message}");
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn trace_spawn_loader_phase(phase: &str) {
    static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_SPAWN_TRACE").is_some()) {
        let started = STARTED.get_or_init(std::time::Instant::now);
        eprintln!(
            "kinakaze loader: pid={} phase={} elapsed_us={}",
            std::process::id(),
            phase,
            started.elapsed().as_micros()
        );
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn report_traps_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_REPORT_TRAPS").is_some())
}

/// Optional one-byte breakpoint for narrowing guest startup without a native
/// debugger. It is completely dormant unless an exact mapped address is supplied.
#[cfg(all(windows, target_arch = "x86_64"))]
fn install_requested_guest_breakpoint() -> Result<(), kinakaze_link::LinkError> {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let Some(address) = std::env::var("KINAKAZE_BREAK_GUEST_ADDRESS")
        .ok()
        .and_then(|value| usize::from_str_radix(value.trim().trim_start_matches("0x"), 16).ok())
    else {
        return Ok(());
    };
    if address == 0 {
        return Err(kinakaze_link::LinkError::AddressOverflow);
    }
    let mut old = 0u32;
    if unsafe {
        VirtualProtect(
            address as *const core::ffi::c_void,
            1,
            PAGE_EXECUTE_READWRITE,
            &mut old,
        )
    } == 0
    {
        return Err(kinakaze_link::LinkError::ProtectionFailed {
            object: format!("<guest breakpoint {address:#x}>"),
        });
    }
    unsafe {
        (address as *mut u8).write_volatile(0xcc);
        FlushInstructionCache(GetCurrentProcess(), address as *const core::ffi::c_void, 1);
    }
    let mut ignored = 0u32;
    if unsafe { VirtualProtect(address as *const core::ffi::c_void, 1, old, &mut ignored) } == 0 {
        return Err(kinakaze_link::LinkError::ProtectionFailed {
            object: format!("<guest breakpoint {address:#x}>"),
        });
    }
    Ok(())
}

/// Initializes the calling native thread with the process loader's ELF TLS
/// layout and TEB slots.
///
/// Provider DLLs contain their own Rust statics, so calling a statically linked
/// copy of `kinakaze_tls` from `libpthread.dll` would configure a different TLS
/// registry.  Threads must cross this process-level entry point instead, just as
/// fork crosses the process coordinator exported below.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "system" fn kinakaze_process_initialize_thread_tls() -> i32 {
    match kinakaze_tls::initialize_thread_tls() {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
mod host;
#[cfg(all(windows, target_arch = "x86_64"))]
mod veh;

/// True when the object is a real program rather than a freestanding probe.
///
/// There are three explicit ELF ways to identify a process image: `ET_EXEC`, a
/// `PT_INTERP` program header, or `DF_1_PIE` on an `ET_DYN` image. Static
/// executables deliberately have no interpreter, so using `PT_INTERP` alone would
/// misclassify them as callable shared objects and enter `_start` on the host ABI
/// without a kernel stack. The project's freestanding probes are plain `ET_DYN`
/// objects without `PT_INTERP` or `DF_1_PIE`.
fn elf_expects_kernel_entry(bytes: &[u8]) -> bool {
    let Ok(elf) = kinakaze_elf::ElfFile::parse(bytes) else {
        return false;
    };
    if elf.header().object_type == kinakaze_elf::ET_EXEC {
        return true;
    }
    if elf.program_headers().is_ok_and(|headers| {
        headers
            .iter()
            .any(|header| header.kind == kinakaze_elf::PT_INTERP)
    }) {
        return true;
    }
    elf.dynamic_info()
        .ok()
        .flatten()
        .is_some_and(|dynamic| dynamic.flags_1 & kinakaze_elf::dynamic::DF_1_PIE != 0)
}

#[cfg(test)]
mod kernel_entry_tests {
    use super::elf_expects_kernel_entry;

    fn word(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn dword(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn qword(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn elf(object_type: u16, program_kind: Option<u32>) -> Vec<u8> {
        let mut bytes = vec![0u8; if program_kind.is_some() { 152 } else { 64 }];
        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        word(&mut bytes, 16, object_type);
        word(&mut bytes, 18, kinakaze_elf::EM_X86_64);
        dword(&mut bytes, 20, 1);
        word(&mut bytes, 52, 64);
        if let Some(kind) = program_kind {
            qword(&mut bytes, 32, 64);
            word(&mut bytes, 54, 56);
            word(&mut bytes, 56, 1);
            dword(&mut bytes, 64, kind);
        }
        bytes
    }

    #[test]
    fn static_et_exec_uses_the_kernel_entry_contract() {
        assert!(elf_expects_kernel_entry(&elf(kinakaze_elf::ET_EXEC, None)));
    }

    #[test]
    fn an_interpreted_et_dyn_uses_the_kernel_entry_contract() {
        assert!(elf_expects_kernel_entry(&elf(
            kinakaze_elf::ET_DYN,
            Some(kinakaze_elf::PT_INTERP),
        )));
    }

    #[test]
    fn a_static_pie_uses_the_kernel_entry_contract() {
        let mut bytes = elf(kinakaze_elf::ET_DYN, Some(kinakaze_elf::PT_DYNAMIC));
        // Point the sole PT_DYNAMIC header at two in-file Elf64_Dyn records.
        qword(&mut bytes, 64 + 8, 120);
        qword(&mut bytes, 64 + 32, 32);
        qword(&mut bytes, 64 + 40, 32);
        qword(&mut bytes, 120, kinakaze_elf::dynamic::DT_FLAGS_1 as u64);
        qword(&mut bytes, 128, kinakaze_elf::dynamic::DF_1_PIE);
        assert!(elf_expects_kernel_entry(&bytes));
    }

    #[test]
    fn a_plain_shared_object_keeps_the_freestanding_probe_contract() {
        assert!(!elf_expects_kernel_entry(&elf(kinakaze_elf::ET_DYN, None)));
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn init_console_modes() {
    use windows_sys::Win32::System::Console::{
        ENABLE_EXTENDED_FLAGS, ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT,
        ENABLE_QUICK_EDIT_MODE, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        GetConsoleMode, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        SetConsoleCP, SetConsoleMode, SetConsoleOutputCP,
    };
    if std::env::var_os("TERM").is_none() {
        unsafe { std::env::set_var("TERM", "xterm-256color") };
    }
    unsafe {
        let _ = SetConsoleCP(65001);
        let _ = SetConsoleOutputCP(65001);
        let h_out = GetStdHandle(STD_OUTPUT_HANDLE);
        if !h_out.is_null() && h_out as isize != -1 {
            let mut mode = 0u32;
            if GetConsoleMode(h_out, &raw mut mode) != 0 {
                let _ = SetConsoleMode(
                    h_out,
                    mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
                );
            }
        }
        let h_err = GetStdHandle(STD_ERROR_HANDLE);
        if !h_err.is_null() && h_err as isize != -1 {
            let mut mode = 0u32;
            if GetConsoleMode(h_err, &raw mut mode) != 0 {
                let _ = SetConsoleMode(
                    h_err,
                    mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
                );
            }
        }
        let h_in = GetStdHandle(STD_INPUT_HANDLE);
        if !h_in.is_null() && h_in as isize != -1 {
            let mut mode = 0u32;
            if GetConsoleMode(h_in, &raw mut mode) != 0 {
                use windows_sys::Win32::System::Console::{
                    ENABLE_MOUSE_INPUT, ENABLE_WINDOW_INPUT,
                };
                let clean_mode = (mode | ENABLE_PROCESSED_INPUT)
                    & !(ENABLE_MOUSE_INPUT
                        | ENABLE_WINDOW_INPUT
                        | ENABLE_QUICK_EDIT_MODE
                        | ENABLE_VIRTUAL_TERMINAL_INPUT)
                    | ENABLE_EXTENDED_FLAGS;
                let _ = SetConsoleMode(h_in, clean_mode);
            }
        }
    }
}

/// A launch is explicit: V2 supplies the executable, Linux argv/environment,
/// guest filesystem root and the already-initialized DLL/ELF provider registry.
/// The runtime session must remain live until native process termination.
pub struct GuestConfig {
    pub executable: std::path::PathBuf,
    pub arguments: Vec<String>,
    pub root: std::path::PathBuf,
    pub cwd: String,
    pub environment: Option<Vec<String>>,
    pub providers: std::sync::Arc<ProviderRegistry>,
}

impl GuestConfig {
    fn validate(&self) -> Result<(), &'static str> {
        if !self.root.is_absolute() || !self.executable.is_absolute() {
            return Err("guest root and executable must be absolute host paths");
        }
        if !self.cwd.starts_with('/') || self.cwd.contains('\0') {
            return Err("guest cwd must be an absolute Linux pathname");
        }
        if self.arguments.iter().any(|value| value.contains('\0'))
            || self
                .environment
                .as_ref()
                .is_some_and(|values| values.iter().any(|value| value.contains('\0')))
        {
            return Err("guest argv and environment cannot contain NUL");
        }
        Ok(())
    }
}

static CONFIGURATION: std::sync::OnceLock<GuestConfig> = std::sync::OnceLock::new();

fn configuration() -> Result<&'static GuestConfig, kinakaze_link::LinkError> {
    CONFIGURATION.get().ok_or_else(|| {
        kinakaze_link::LinkError::InvalidProvider("guest engine has not been configured".to_owned())
    })
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn hosted_main() -> i32 {
    trace_spawn_loader_phase("hosted-main-entered");
    init_console_modes();
    let config = match configuration() {
        Ok(config) => config,
        Err(error) => {
            report_loader_error(format_args!("guest configuration failed: {error}"));
            return 1;
        }
    };
    kinakaze_vfs::set_system_root(config.root.clone());
    if let Err(errno) = kinakaze_vfs::fs::chdir(&config.cwd) {
        report_loader_error(format_args!(
            "cannot change guest directory to {:?}: errno {errno}",
            config.cwd
        ));
        return 1;
    }
    let path = config.executable.clone();
    let guest_arguments = if config.arguments.is_empty() {
        vec![kinakaze_vfs::to_guest_path(&path)]
    } else {
        config.arguments.clone()
    };
    let original_path = path.clone();
    // Both execve and posix_spawn publish a PID-scoped handoff. The private
    // `--kinakaze-exec` argument distinguishes execve's argv layout, not whether
    // a handoff exists; probing the mapping is the authoritative test and also
    // lets posix_spawn consume the already-pinned executable image.
    let shared_launch = match kinakaze_vfs::peek_exec_launch_state() {
        Ok(state) => state,
        Err(()) => {
            report_loader_error(format_args!("ELF exec handoff is malformed"));
            return 1;
        }
    };
    trace_spawn_loader_phase("exec-image-peeked");
    if shared_launch.is_some() {
        if let Err(error) = host::consume_exec_handoff() {
            report_loader_error(format_args!(
                "ELF exec handoff initialization failed: {error}"
            ));
            return 1;
        }
        trace_spawn_loader_phase("libc-handoff-consumed");
    }
    let (path, guest_arguments) = {
        let mut shebang_buf = [0u8; 1024];
        let first_bytes =
            if let Some(image) = shared_launch.as_ref().map(|state| state.image.as_slice()) {
                Some(&image[..image.len().min(shebang_buf.len())])
            } else if let Ok(mut file) = kinakaze_link::open_guest_image(&path) {
                use std::io::Read;
                file.read(&mut shebang_buf)
                    .ok()
                    .map(|read| &shebang_buf[..read])
            } else {
                None
            };
        let shebang_opt = if let Some(first_bytes) = first_bytes {
            if first_bytes.starts_with(b"#!") {
                if let Some(newline_pos) =
                    first_bytes.iter().position(|&b| b == b'\n' || b == b'\r')
                {
                    let shebang_line = String::from_utf8_lossy(&first_bytes[2..newline_pos]);
                    let parts: Vec<&str> = shebang_line.split_whitespace().collect();
                    if let Some(interp) = parts.first() {
                        let interp_host = kinakaze_vfs::resolve_linux_path(interp)
                            .unwrap_or_else(|_| std::path::PathBuf::from(interp));
                        let mut new_args = Vec::new();
                        new_args.push(interp.to_string());
                        for extra in &parts[1..] {
                            new_args.push(extra.to_string());
                        }
                        new_args.push(kinakaze_vfs::to_guest_path(&path));
                        if guest_arguments.len() > 1 {
                            new_args.extend(guest_arguments[1..].iter().cloned());
                        }
                        Some((interp_host, new_args))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        shebang_opt.unwrap_or((path, guest_arguments))
    };
    // A shebang selects a different root ELF. The shared bytes describe the
    // script itself, so only the unchanged path may consume that snapshot.
    let (root_image, exec_environment) = match shared_launch {
        Some(state) => (
            (path == original_path).then_some(state.image),
            Some(state.environment),
        ),
        None => (None, config.environment.clone()),
    };

    // Read once into immutable backing, shared by entry classification and the
    // linker. A temporary Vec here would still inflate every later fork copy.
    let root_image = match root_image {
        Some(image) => image,
        None => match kinakaze_link::snapshot_guest_image(&path) {
            Ok(image) => image,
            Err(error) => {
                report_loader_error(format_args!("ELF image snapshot failed: {error}"));
                return 1;
            }
        },
    };

    // Kept outside the guest-visible environment by `host::exec`; libc uses it
    // solely to implement Linux `/proc/self/exe` across a hosted exec.
    unsafe { std::env::set_var("KINAKAZE_GUEST_EXECUTABLE", &path) };
    let guest_executable = kinakaze_vfs::to_guest_path(&path);
    let comm_source = guest_arguments
        .first()
        .map(String::as_str)
        .unwrap_or(guest_executable.as_str());
    let comm = comm_source
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("kinakaze");
    kinakaze_vfs::job::set_process_identity(comm, &guest_executable, &guest_arguments);
    trace_spawn_loader_phase("guest-identity-set");

    if elf_expects_kernel_entry(&root_image) {
        return match host::exec(&path, &guest_arguments, Some(root_image), exec_environment) {
            // `exec` returns Infallible on success, so only the error arm exists.
            Ok(never) => match never {},
            Err(error) => {
                report_loader_error(format_args!(
                    "ELF exec failed for {}: {error}",
                    path.display()
                ));
                1
            }
        };
    }

    match host::run(&path, Some(root_image)) {
        Ok(status) => {
            println!("ELF entry returned {status}");
            status as i32
        }
        Err(error) => {
            report_loader_error(format_args!(
                "ELF load failed for {}: {error}",
                path.display()
            ));
            1
        }
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[derive(Clone, Copy)]
#[repr(C)]
struct EngineForkSnapshot {
    canary_target: usize,
    thread_pointer_target: usize,
    host_transition_target: usize,
    scratch_target: usize,
    canary_value: usize,
    thread_pointer_value: usize,
    host_transition_value: usize,
    scratch_value: usize,
    host_transition_state: [u8; kinakaze_tls::thread_pointer::HOST_TRANSITION_BLOCK_SIZE],
    stack_base: usize,
    stack_limit: usize,
    syscall_dispatcher: usize,
    synchronous_signal_dispatcher: usize,
    mapping_fault_dispatcher: usize,
    report_child_traps: usize,
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn teb_slot_value(target: usize) -> Option<usize> {
    use windows_sys::Win32::System::Threading::TlsGetValue;

    let slot = crate::execution::segment_patch::teb_slot_index(target)?;
    // SAFETY: the validated index is one of the 64 slots stored directly in
    // the current thread's TEB.
    Some(unsafe { TlsGetValue(slot) as usize })
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn restore_teb_slot(target: usize, value: usize) -> bool {
    use windows_sys::Win32::System::Threading::TlsSetValue;

    let Some(slot) = crate::execution::segment_patch::teb_slot_index(target) else {
        return false;
    };
    // SAFETY: the validated index names a direct TEB slot in this thread.
    unsafe { TlsSetValue(slot, value as *mut core::ffi::c_void) != 0 }
}

/// Process-level Linux FS-base publication used by provider DLLs.
///
/// Every PE module has an independent copy of Rust statics, while the direct
/// TEB slot consumed by generated code belongs to the process loader.  Keep the
/// slot authoritative and let libc/libpthread cross this narrow ABI instead of
/// accidentally updating a module-private TLS registry.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_process_set_thread_pointer(value: usize) -> i32 {
    let target = host::thread_pointer_target();
    let before = teb_slot_value(target).unwrap_or(0);
    let result = if target == 0 || !restore_teb_slot(target, value) {
        22
    } else {
        0
    };
    if std::env::var_os("KINAKAZE_FORK_TRACE").is_some() {
        eprintln!(
            "kinakaze loader: publish thread pointer target={target:#x} before={before:#x} value={value:#x} after={:#x} result={result}",
            teb_slot_value(target).unwrap_or(0),
        );
    }
    result
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_process_get_thread_pointer() -> usize {
    teb_slot_value(host::thread_pointer_target()).unwrap_or(0)
}

/// Returns the process-owned host transition block for the calling thread.
/// Provider DLLs use this export instead of their module-private Rust statics.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_process_get_host_transition_pointer() -> usize {
    teb_slot_value(host::host_transition_target()).unwrap_or(0)
}

/// General register state preserved by Linux across a raw syscall. RAX is the
/// return value, while RCX and R11 are syscall-clobbered and are synthesized by
/// [`resume_raw_clone_child`].
#[cfg(all(windows, target_arch = "x86_64"))]
#[derive(Clone, Copy)]
#[repr(C)]
struct RawCloneGuestContext {
    rsp: usize,
    rip: usize,
    rflags: usize,
    rdi: usize,
    rsi: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
    r10: usize,
    rbp: usize,
    rbx: usize,
    r12: usize,
    r13: usize,
    r14: usize,
    r15: usize,
}

#[cfg(all(windows, target_arch = "x86_64"))]
type RawCloneThreadInitializer = unsafe extern "sysv64" fn(u64, *mut i32, usize, u64, usize) -> i32;

#[cfg(all(windows, target_arch = "x86_64"))]
#[repr(C)]
struct RawCloneThreadPacket {
    context: RawCloneGuestContext,
    flags: u64,
    child_tid: *mut i32,
    thread_pointer: usize,
    gs_base: usize,
    signal_mask: u64,
    initializer: RawCloneThreadInitializer,
    stack_base: usize,
    stack_limit: usize,
    ready: windows_sys::Win32::Foundation::HANDLE,
    status: std::sync::atomic::AtomicI32,
    inheritance: usize,
}

#[cfg(all(windows, target_arch = "x86_64"))]
unsafe impl Send for RawCloneThreadPacket {}

#[cfg(all(windows, target_arch = "x86_64"))]
unsafe fn read_raw_syscall_frame(frame: usize, offset: usize) -> usize {
    // SAFETY: callers obtain `frame` from the active trampoline and every named
    // offset is part of its fixed 192-byte register record.
    unsafe { ((frame + offset) as *const usize).read_unaligned() }
}

/// Restores a clone child directly at the instruction following the intercepted
/// Linux syscall. The Windows entry stack remains allocated for host unwind and
/// TLS bookkeeping; guest RSP is selected only after all initialization and the
/// parent handshake have completed.
#[cfg(all(windows, target_arch = "x86_64"))]
unsafe fn resume_raw_clone_child(context: &RawCloneGuestContext) -> ! {
    unsafe {
        core::arch::asm!(
            "mov rax, {context}",
            "push qword ptr [rax + {rflags}]",
            "popfq",
            "mov rdi, [rax + {rdi}]",
            "mov rsi, [rax + {rsi}]",
            "mov rdx, [rax + {rdx}]",
            "mov r8, [rax + {r8}]",
            "mov r9, [rax + {r9}]",
            "mov r10, [rax + {r10}]",
            "mov rbp, [rax + {rbp}]",
            "mov rbx, [rax + {rbx}]",
            "mov r12, [rax + {r12}]",
            "mov r13, [rax + {r13}]",
            "mov r14, [rax + {r14}]",
            "mov r15, [rax + {r15}]",
            "mov rsp, [rax + {rsp}]",
            "mov rcx, [rax + {rip}]",
            "mov r11, [rax + {rflags}]",
            "mov rax, 0",
            "jmp rcx",
            context = in(reg) context,
            rsp = const core::mem::offset_of!(RawCloneGuestContext, rsp),
            rip = const core::mem::offset_of!(RawCloneGuestContext, rip),
            rflags = const core::mem::offset_of!(RawCloneGuestContext, rflags),
            rdi = const core::mem::offset_of!(RawCloneGuestContext, rdi),
            rsi = const core::mem::offset_of!(RawCloneGuestContext, rsi),
            rdx = const core::mem::offset_of!(RawCloneGuestContext, rdx),
            r8 = const core::mem::offset_of!(RawCloneGuestContext, r8),
            r9 = const core::mem::offset_of!(RawCloneGuestContext, r9),
            r10 = const core::mem::offset_of!(RawCloneGuestContext, r10),
            rbp = const core::mem::offset_of!(RawCloneGuestContext, rbp),
            rbx = const core::mem::offset_of!(RawCloneGuestContext, rbx),
            r12 = const core::mem::offset_of!(RawCloneGuestContext, r12),
            r13 = const core::mem::offset_of!(RawCloneGuestContext, r13),
            r14 = const core::mem::offset_of!(RawCloneGuestContext, r14),
            r15 = const core::mem::offset_of!(RawCloneGuestContext, r15),
            options(noreturn)
        )
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
unsafe extern "system" fn raw_clone_thread_start(parameter: *mut core::ffi::c_void) -> u32 {
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::System::Threading::SetEvent;

    // Copy everything before signalling readiness: the parent owns and frees the
    // packet immediately after observing that event.
    let packet = unsafe { &*(parameter as *const RawCloneThreadPacket) };
    let context = packet.context;
    let flags = packet.flags;
    let child_tid = packet.child_tid;
    let thread_pointer = packet.thread_pointer;
    let gs_base = packet.gs_base;
    let signal_mask = packet.signal_mask;
    let initializer = packet.initializer;
    let stack_base = packet.stack_base;
    let stack_limit = packet.stack_limit;
    let ready = packet.ready;
    let trace = std::env::var_os("KINAKAZE_CLONE_TRACE").is_some();
    if trace {
        eprintln!(
            "kinakaze loader: clone child enter stack={:#x} rip={:#x} tp={thread_pointer:#x} flags={flags:#x}",
            context.rsp, context.rip,
        );
    }

    let mut status = kinakaze_process_initialize_thread_tls();
    if status == 0 && !kinakaze_tls::set_guest_gs_base(gs_base) {
        status = 22;
    }
    if status == 0 && kinakaze_tls::thread_pointer::restore_thread_pointer(thread_pointer).is_err()
    {
        status = 22;
    }
    if status == 0 {
        // SAFETY: the provider passed a process-lifetime function with this ABI.
        status = unsafe {
            initializer(
                flags,
                child_tid,
                thread_pointer,
                signal_mask,
                packet.inheritance,
            )
        };
    }
    if status == 0 {
        // The active guest stack is not a Windows-created stack, so describe its
        // real committed allocation before exception dispatch can observe it.
        unsafe {
            core::arch::asm!(
                "mov gs:[0x08], {base}",
                "mov gs:[0x10], {limit}",
                base = in(reg) stack_base,
                limit = in(reg) stack_limit,
                options(nostack, preserves_flags),
            );
        }
    }
    packet.status.store(status, Ordering::Release);
    if trace {
        eprintln!("kinakaze loader: clone child initialized status={status}");
    }
    // SAFETY: `ready` remains owned by the parent until this signal is observed.
    unsafe { SetEvent(ready) };
    if status != 0 {
        return status as u32;
    }
    // SAFETY: initialization completed and the context is a synchronous copy of
    // the parent's live raw-syscall frame.
    unsafe { resume_raw_clone_child(&context) }
}

/// Creates a Linux thread-shaped raw clone and returns its kernel-visible tid.
///
/// Flag validation and Linux per-thread state live in libc; the executable owns
/// only address mappings, TEB slots and the returns-twice register transition.
///
/// # Safety
/// The caller supplies a valid guest stack and TLS block, a matching runtime
/// initializer, and writable TID locations required by the validated clone
/// flags. The inheritance record must remain live until initialization finishes.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_clone_thread(
    child_stack: usize,
    thread_pointer: usize,
    initializer: usize,
    flags: u64,
    parent_tid: *mut i32,
    child_tid: *mut i32,
    signal_mask: u64,
    inheritance: usize,
) -> i64 {
    let limit_error = kinakaze_runtime::services::task_creation_errno();
    if limit_error != 0 {
        return -(limit_error as i64);
    }
    use std::sync::atomic::{AtomicI32, Ordering};
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_FAILED, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
        PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, CreateEventW, CreateThread, INFINITE, ResumeThread, TerminateThread,
        WaitForMultipleObjects, WaitForSingleObject,
    };

    const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
    let trace = std::env::var_os("KINAKAZE_CLONE_TRACE").is_some();
    if trace {
        eprintln!(
            "kinakaze loader: clone host enter stack={child_stack:#x} tp={thread_pointer:#x} initializer={initializer:#x} flags={flags:#x} parent_tid={parent_tid:p} child_tid={child_tid:p}"
        );
    }
    if child_stack == 0 || thread_pointer == 0 || initializer == 0 {
        if trace {
            eprintln!("kinakaze loader: clone host rejected invalid required argument");
        }
        return -22;
    }
    let mut memory = MEMORY_BASIC_INFORMATION::default();
    let queried = unsafe {
        VirtualQuery(
            (child_stack - 1) as *const core::ffi::c_void,
            &mut memory,
            core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if queried == 0 || memory.State != MEM_COMMIT || memory.AllocationBase.is_null() {
        return -14;
    }
    if flags & CLONE_PARENT_SETTID != 0 {
        if parent_tid.is_null() {
            return -14;
        }
        let mut parent_memory = MEMORY_BASIC_INFORMATION::default();
        let queried = unsafe {
            VirtualQuery(
                parent_tid.cast(),
                &mut parent_memory,
                core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        let protection = parent_memory.Protect & 0xff;
        let writable = matches!(
            protection,
            PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
        );
        let region_end = (parent_memory.BaseAddress as usize)
            .checked_add(parent_memory.RegionSize)
            .unwrap_or(0);
        if queried == 0
            || parent_memory.State != MEM_COMMIT
            || !writable
            || (parent_tid as usize)
                .checked_add(core::mem::size_of::<i32>())
                .is_none_or(|end| end > region_end)
        {
            return -14;
        }
    }
    let initializer: RawCloneThreadInitializer = unsafe { core::mem::transmute(initializer) };
    let context = if let (Some(frame), Some((_, logical_rip))) = (
        kinakaze_tls::thread_pointer::active_raw_syscall_frame_pointer(),
        kinakaze_tls::thread_pointer::active_guest_signal_context(),
    ) {
        let rip = unsafe {
            read_raw_syscall_frame(
                frame,
                kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_CONTINUATION_OFFSET,
            )
        };
        if rip == 0 {
            return -22;
        }
        if trace {
            eprintln!(
                "kinakaze loader: clone host direct frame={frame:#x} logical rip={logical_rip:#x} translated rip={rip:#x}"
            );
        }
        let read = |offset| unsafe { read_raw_syscall_frame(frame, offset) };
        RawCloneGuestContext {
            rsp: child_stack,
            rip,
            // User-visible condition flags after a syscall are not an input to
            // its ABI; IF and the reserved bit are the SYSRET baseline.
            rflags: 0x202,
            rdi: read(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_RDI_OFFSET),
            rsi: read(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_RSI_OFFSET),
            rdx: read(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_RDX_OFFSET),
            r8: read(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R8_OFFSET),
            r9: read(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R9_OFFSET),
            r10: read(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R10_OFFSET),
            rbp: read(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_RBP_OFFSET),
            rbx: read(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_RBX_OFFSET),
            r12: read(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R12_OFFSET),
            r13: read(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R13_OFFSET),
            r14: read(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R14_OFFSET),
            r15: read(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R15_OFFSET),
        }
    } else {
        let context_pointer = host::active_trap_context_pointer();
        if context_pointer == 0 {
            return -22;
        }
        // SAFETY: this process export is called synchronously from the vectored
        // handler while the per-thread Windows CONTEXT remains live.
        let trap = unsafe {
            &*(context_pointer as *const windows_sys::Win32::System::Diagnostics::Debug::CONTEXT)
        };
        RawCloneGuestContext {
            rsp: child_stack,
            rip: (trap.Rip as usize).saturating_add(2),
            rflags: trap.EFlags as usize,
            rdi: trap.Rdi as usize,
            rsi: trap.Rsi as usize,
            rdx: trap.Rdx as usize,
            r8: trap.R8 as usize,
            r9: trap.R9 as usize,
            r10: trap.R10 as usize,
            rbp: trap.Rbp as usize,
            rbx: trap.Rbx as usize,
            r12: trap.R12 as usize,
            r13: trap.R13 as usize,
            r14: trap.R14 as usize,
            r15: trap.R15 as usize,
        }
    };
    // Serialize native thread publication with fork's complete topology
    // snapshot, using the same gate as pthread_create and guest mmap mutations.
    let Some(creation_transaction) = kinakaze_runtime::begin_fork_mapping_transaction() else {
        return -11;
    };
    let ready = unsafe { CreateEventW(core::ptr::null(), 0, 0, core::ptr::null()) };
    if ready.is_null() {
        return -11;
    }
    let packet = Box::new(RawCloneThreadPacket {
        inheritance,
        context,
        flags,
        child_tid,
        thread_pointer,
        gs_base: kinakaze_tls::guest_gs_base(),
        signal_mask,
        initializer,
        stack_base: child_stack,
        stack_limit: memory.AllocationBase as usize,
        ready,
        status: AtomicI32::new(11),
    });
    let packet = Box::into_raw(packet);
    let mut thread_id = 0u32;
    let thread = unsafe {
        CreateThread(
            core::ptr::null(),
            0,
            Some(raw_clone_thread_start),
            packet.cast(),
            CREATE_SUSPENDED,
            &mut thread_id,
        )
    };
    if thread.is_null() {
        unsafe {
            drop(Box::from_raw(packet));
            CloseHandle(ready);
        }
        return -11;
    }
    // Linux completes CLONE_PARENT_SETTID before the new task can execute any
    // userspace instruction. Keep the native thread suspended through this
    // publication; writing it after the readiness handshake lets a short-lived
    // child exit and release its pthread storage before the parent store.
    let previous_parent_tid = if flags & CLONE_PARENT_SETTID != 0 {
        let previous = unsafe { parent_tid.read() };
        unsafe { parent_tid.write(thread_id as i32) };
        Some(previous)
    } else {
        None
    };
    if unsafe { ResumeThread(thread) } == u32::MAX {
        if let Some(previous) = previous_parent_tid {
            unsafe { parent_tid.write(previous) };
        }
        unsafe {
            TerminateThread(thread, 11);
            WaitForSingleObject(thread, INFINITE);
            drop(Box::from_raw(packet));
            CloseHandle(ready);
            CloseHandle(thread);
        }
        return -11;
    }
    if trace {
        eprintln!(
            "kinakaze loader: clone host created handle={thread:p} native tid={thread_id} ready={ready:p}"
        );
    }

    // A child initializer may mutate VM/TLS mappings. Once the new thread is
    // visible to the freezer, release the gate before waiting for readiness.
    drop(creation_transaction);
    // Readiness and premature death are waited together. There is no timeout:
    // initialization is in-process and a dead child is an immediate failure,
    // while an arbitrary deadline would introduce the race it tries to hide.
    let handles = [ready, thread];
    let waited = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
    let status = unsafe { (*packet).status.load(Ordering::Acquire) };
    if trace {
        eprintln!(
            "kinakaze loader: clone host wait result={waited:#x} status={status} native tid={thread_id}"
        );
    }
    unsafe {
        drop(Box::from_raw(packet));
        CloseHandle(ready);
        CloseHandle(thread);
    }
    let result = if waited == WAIT_FAILED {
        -11
    } else if waited == WAIT_OBJECT_0 && status == 0 {
        i64::from(thread_id)
    } else {
        -i64::from(status.max(1))
    };
    if trace {
        eprintln!("kinakaze loader: clone host return={result} ({result:#x})");
    }
    result
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn engine_teb_slots_reserved_in_child(snapshot: &EngineForkSnapshot) -> bool {
    let slots = [
        crate::execution::segment_patch::teb_slot_index(snapshot.canary_target),
        crate::execution::segment_patch::teb_slot_index(snapshot.thread_pointer_target),
        crate::execution::segment_patch::teb_slot_index(snapshot.host_transition_target),
        crate::execution::segment_patch::teb_slot_index(snapshot.scratch_target),
    ];
    let [
        Some(canary),
        Some(thread_pointer),
        Some(host_transition),
        Some(scratch),
    ] = slots
    else {
        return false;
    };
    let slots = [canary, thread_pointer, host_transition, scratch];

    kinakaze_runtime::fork_tls_slots_registered(&slots)
}

#[cfg(all(windows, target_arch = "x86_64"))]
unsafe extern "system" fn snapshot_engine_fork_state(buffer: *mut u8, len: usize) -> isize {
    let redirects = execution::guest_gs::snapshot();
    let header = core::mem::size_of::<EngineForkSnapshot>();
    let needed = header + redirects.len();
    if buffer.is_null() {
        return needed as isize;
    }
    if len < needed {
        return -1;
    }
    let thread_pointer_value = teb_slot_value(host::thread_pointer_target()).unwrap_or(0);
    let host_transition_value = teb_slot_value(host::host_transition_target()).unwrap_or(0);
    let mut host_transition_state = [0u8; kinakaze_tls::thread_pointer::HOST_TRANSITION_BLOCK_SIZE];
    if host_transition_value != 0 {
        // SAFETY: the configured slot points at the complete loader-owned block.
        unsafe {
            core::ptr::copy_nonoverlapping(
                host_transition_value as *const u8,
                host_transition_state.as_mut_ptr(),
                host_transition_state.len(),
            )
        };
    }
    let (stack_base, stack_limit): (usize, usize);
    unsafe {
        core::arch::asm!(
            "mov {base}, gs:[0x08]",
            "mov {limit}, gs:[0x10]",
            base = out(reg) stack_base,
            limit = out(reg) stack_limit,
            options(nostack, preserves_flags),
        );
    }
    let snapshot = EngineForkSnapshot {
        canary_target: host::canary_target(),
        thread_pointer_target: host::thread_pointer_target(),
        host_transition_target: host::host_transition_target(),
        scratch_target: host::scratch_target(),
        canary_value: teb_slot_value(host::canary_target()).unwrap_or(0),
        thread_pointer_value,
        host_transition_value,
        scratch_value: teb_slot_value(host::scratch_target()).unwrap_or(0),
        host_transition_state,
        stack_base,
        stack_limit,
        syscall_dispatcher: host::syscall_dispatcher(),
        synchronous_signal_dispatcher: host::synchronous_signal_dispatcher(),
        mapping_fault_dispatcher: host::mapping_fault_dispatcher(),
        report_child_traps: usize::from(
            std::env::var_os("KINAKAZE_REPORT_FORK_CHILD_TRAPS").is_some(),
        ),
    };
    if std::env::var_os("KINAKAZE_FORK_TRACE").is_some() {
        let host_stack_top = usize::from_ne_bytes(
            snapshot.host_transition_state
                [kinakaze_tls::thread_pointer::HOST_CALL_STACK_POINTER_OFFSET
                    ..kinakaze_tls::thread_pointer::HOST_CALL_STACK_POINTER_OFFSET + 8]
                .try_into()
                .unwrap(),
        );
        let saved_guest_r14 = if host_stack_top >= 192 {
            unsafe { ((host_stack_top - 192 + 0x90) as *const usize).read_unaligned() }
        } else {
            0
        };
        let guest_tls_g = if snapshot.thread_pointer_value >= core::mem::size_of::<usize>() {
            unsafe {
                ((snapshot.thread_pointer_value - core::mem::size_of::<usize>()) as *const usize)
                    .read_unaligned()
            }
        } else {
            0
        };
        eprintln!(
            "kinakaze loader: snapshot teb canary target={:#x} value={:#x} tp target={:#x} value={:#x} guest_tls_g={guest_tls_g:#x} active_rsp={:#x} active_rip={:#x} host_stack_top={host_stack_top:#x} saved_guest_r14={saved_guest_r14:#x} stack_base={:#x} stack_limit={:#x}",
            snapshot.canary_target,
            snapshot.canary_value,
            snapshot.thread_pointer_target,
            snapshot.thread_pointer_value,
            usize::from_ne_bytes(
                snapshot.host_transition_state
                    [kinakaze_tls::thread_pointer::ACTIVE_GUEST_STACK_POINTER_OFFSET
                        ..kinakaze_tls::thread_pointer::ACTIVE_GUEST_STACK_POINTER_OFFSET + 8]
                    .try_into()
                    .unwrap(),
            ),
            usize::from_ne_bytes(
                snapshot.host_transition_state
                    [kinakaze_tls::thread_pointer::ACTIVE_GUEST_INSTRUCTION_POINTER_OFFSET
                        ..kinakaze_tls::thread_pointer::ACTIVE_GUEST_INSTRUCTION_POINTER_OFFSET
                            + 8]
                    .try_into()
                    .unwrap(),
            ),
            snapshot.stack_base,
            snapshot.stack_limit,
        );
    }
    unsafe {
        *(buffer as *mut EngineForkSnapshot) = snapshot;
        core::ptr::copy_nonoverlapping(redirects.as_ptr(), buffer.add(header), redirects.len());
    }
    needed as isize
}

#[cfg(all(windows, target_arch = "x86_64"))]
unsafe extern "system" fn restore_engine_fork_state(payload: *const u8, len: usize) -> i32 {
    if !payload.is_null() && len >= core::mem::size_of::<EngineForkSnapshot>() {
        let header = core::mem::size_of::<EngineForkSnapshot>();
        if !execution::guest_gs::restore(unsafe {
            core::slice::from_raw_parts(payload.add(header), len - header)
        }) {
            return 22;
        }
        let snapshot = unsafe { *(payload as *const EngineForkSnapshot) };
        // Restore the generated-code transition state before rebuilding any
        // private loader collections. Native DLLs have fresh host heaps.
        host::install_fault_reporter(
            snapshot.canary_target,
            snapshot.thread_pointer_target,
            snapshot.host_transition_target,
            snapshot.scratch_target,
        );
        host::set_trap_reporting(snapshot.report_child_traps != 0);
        host::set_syscall_dispatcher(snapshot.syscall_dispatcher);
        host::set_synchronous_signal_dispatcher(snapshot.synchronous_signal_dispatcher);
        host::set_mapping_fault_dispatcher(snapshot.mapping_fault_dispatcher);
        let Some(canary_slot) =
            crate::execution::segment_patch::teb_slot_index(snapshot.canary_target)
        else {
            return 22;
        };
        let Some(thread_pointer_slot) =
            crate::execution::segment_patch::teb_slot_index(snapshot.thread_pointer_target)
        else {
            return 22;
        };
        let Some(host_transition_slot) =
            crate::execution::segment_patch::teb_slot_index(snapshot.host_transition_target)
        else {
            return 22;
        };
        if !engine_teb_slots_reserved_in_child(&snapshot)
            || kinakaze_tls::configure_canary_teb_slot(canary_slot, snapshot.canary_value).is_err()
            || kinakaze_tls::configure_static_tls_teb_slot(thread_pointer_slot).is_err()
            || kinakaze_tls::configure_host_transition_teb_slot(host_transition_slot).is_err()
            || !restore_teb_slot(snapshot.canary_target, snapshot.canary_value)
            || !restore_teb_slot(
                snapshot.thread_pointer_target,
                snapshot.thread_pointer_value,
            )
            || !restore_teb_slot(
                snapshot.host_transition_target,
                snapshot.host_transition_value,
            )
            || !restore_teb_slot(snapshot.scratch_target, snapshot.scratch_value)
        {
            return 22;
        }
        if snapshot.host_transition_value != 0 {
            // Restore the complete registered internal state before guest code
            // resumes. This includes shadow-return stacks and host-stack top, not
            // only the signal handoff fields that happened to fail first.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    snapshot.host_transition_state.as_ptr(),
                    snapshot.host_transition_value as *mut u8,
                    snapshot.host_transition_state.len(),
                )
            }
        }
        // These are per-thread ABI-transition state, just like a fibre's stack
        // bounds.  The child starts with a fresh Windows TEB, so restoring only
        // the copied stack bytes would leave exception dispatch and stack probes
        // describing the bootstrap thread instead of the resumed guest thread.
        unsafe {
            core::arch::asm!(
                "mov gs:[0x08], {base}",
                "mov gs:[0x10], {limit}",
                base = in(reg) snapshot.stack_base,
                limit = in(reg) snapshot.stack_limit,
                options(nostack, preserves_flags),
            );
        }
    } else {
        return 22;
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn register_engine_fork_participant() -> bool {
    kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        // The loader owns the process-wide TEB publications and ABI transition
        // block that provider participants depend on. POSIX child handlers run
        // in registration order (the inverse of prepare), so restore this
        // substrate before any DLL-local state is adopted.
        priority: 0,
        key: 0x454e_4741_4249_3031, // "ENGABI01"
        prepare: None,
        snapshot: Some(snapshot_engine_fork_state),
        parent: None,
        child: Some(restore_engine_fork_state),
    })
}

/// Enter one Linux process after the outer runtime has authenticated and adopted
/// this native worker. Success enters ELF `_start` and exits through Linux exit;
/// returning an integer reports startup failure or a freestanding test result.
///
/// There is one engine launch per native worker. Reusing a worker would retain
/// process-local ELF/TLS/mapping state and is therefore rejected explicitly.
#[cfg(all(windows, target_arch = "x86_64"))]
pub fn run(config: GuestConfig) -> i32 {
    if let Err(error) = config.validate() {
        report_loader_error(format_args!("{error}"));
        return 2;
    }
    if let Err(error) = kinakaze_vfs::path::initialize_namespace_root(config.root.clone()) {
        report_loader_error(format_args!(
            "cannot configure namespace root: errno {error}"
        ));
        return 2;
    }
    if CONFIGURATION.set(config).is_err() {
        report_loader_error(format_args!(
            "this native worker already owns a guest process"
        ));
        return 2;
    }
    kinakaze_runtime::execution::install(execution::prepare);
    trace_spawn_loader_phase("runtime-entry");
    if !register_engine_fork_participant() {
        report_loader_error(format_args!(
            "cannot register the guest loader fork participant"
        ));
        return 1;
    }
    let status = kinakaze_runtime::run(hosted_main);
    host::publish_loader_exit(status);
    status
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
pub fn run(_config: GuestConfig) -> i32 {
    eprintln!("the ELF execution bridge currently requires x86_64 Windows");
    1
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod launch_tests {
    use super::*;

    fn config() -> GuestConfig {
        GuestConfig {
            executable: r"C:\guest\usr\bin\busybox".into(),
            arguments: vec![
                "/usr/bin/busybox".into(),
                "echo".into(),
                "hello world".into(),
            ],
            root: r"C:\guest".into(),
            cwd: "/".into(),
            environment: Some(vec!["PATH=/usr/bin:/bin".into()]),
            providers: std::sync::Arc::new(ProviderRegistry::new()),
        }
    }

    #[test]
    fn host_locations_and_guest_cwd_are_distinct_path_contracts() {
        let mut config = config();
        assert!(config.validate().is_ok());
        config.root = "relative-root".into();
        assert!(config.validate().is_err());
        config.root = r"C:\guest".into();
        config.cwd = r"C:\guest".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn stack_strings_cannot_silently_truncate_at_nul() {
        let mut config = config();
        config.arguments.push("before\0after".into());
        assert!(config.validate().is_err());
        config.arguments.pop();
        config.environment = Some(vec!["KEY=before\0after".into()]);
        assert!(config.validate().is_err());
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
extern "C" fn install_process_services() {
    kinakaze_runtime::services::install_engine(kinakaze_runtime::services::EngineServices {
        clone_thread: kinakaze_process_clone_thread,
    });
}
#[cfg(all(windows, target_arch = "x86_64"))]
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static PROCESS_SERVICES: extern "C" fn() = install_process_services;
