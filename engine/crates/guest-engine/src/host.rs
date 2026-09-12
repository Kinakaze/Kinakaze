//! Guest bootstrap and the dynamic linker entry contract.
use kinakaze_link::process as loader;
use loader::{clear_active_linker, publish_active_linker};
use loader::{
    kinakaze_process_dl_iterate_phdr, kinakaze_process_dladdr, kinakaze_process_dlclose,
    kinakaze_process_dlerror, kinakaze_process_dlinfo, kinakaze_process_dlopen,
    kinakaze_process_dlsym, kinakaze_process_dlvsym,
};
use std::path::{Path, PathBuf};
use std::ptr;

use kinakaze_link::{LinkError, Linker, SearchPaths};

pub(crate) use crate::veh::{
    active_trap_context_pointer, canary_target, host_transition_target, install_fault_reporter,
    mapping_fault_dispatcher, scratch_target, set_mapping_fault_dispatcher,
    set_synchronous_signal_dispatcher, set_syscall_dispatcher, set_trap_reporting,
    synchronous_signal_dispatcher, syscall_dispatcher, thread_pointer_target,
};

/// Symbols the host implements on the guest's behalf.
///
/// These are not ordinary libc entry points: they are the two places where
/// guest code has to call back into the loader itself, so no guest object
/// could provide them.
pub fn register_builtins(linker: &mut Linker) -> Result<(), LinkError> {
    linker.add_builtin("__tls_get_addr", elf_tls_get_addr as *const () as usize);
    linker.add_builtin(
        "__kinakaze_host_write",
        elf_host_write as *const () as usize,
    );
    linker.add_builtin("dlopen", kinakaze_process_dlopen as *const () as usize);
    linker.add_builtin("dlsym", kinakaze_process_dlsym as *const () as usize);
    linker.add_builtin("dlvsym", kinakaze_process_dlvsym as *const () as usize);
    linker.add_builtin("dlclose", kinakaze_process_dlclose as *const () as usize);
    linker.add_builtin("dlerror", kinakaze_process_dlerror as *const () as usize);
    linker.add_builtin("dladdr", kinakaze_process_dladdr as *const () as usize);
    linker.add_builtin("dlinfo", kinakaze_process_dlinfo as *const () as usize);
    linker.add_builtin(
        "dl_iterate_phdr",
        kinakaze_process_dl_iterate_phdr as *const () as usize,
    );
    linker.add_builtin("kinakaze_abi_syscall_raw", process_syscall_dispatcher()?);
    Ok(())
}

/// Every guest, including static ELF, uses the runtime-owned libc engine.
/// No provider is loaded through an ELF pathname and no second state copy exists.
fn process_syscall_dispatcher() -> Result<usize, LinkError> {
    let raw = libc::sysadmin::kinakaze_abi_syscall_raw as *const () as usize;
    set_syscall_dispatcher(raw);
    set_synchronous_signal_dispatcher(
        libc::signal::kinakaze_abi_deliver_synchronous_signal as *const () as usize,
    );
    set_mapping_fault_dispatcher(
        libc::fdio::kinakaze_abi_mapping_fault_signal as *const () as usize,
    );
    kinakaze_link::trap::set_termination_publisher(
        libc::kinakaze_process_publish_loader_termination,
    );
    Ok(raw)
}

/// The two-word `tls_index` object the general-dynamic TLS model passes.
#[repr(C)]
struct TlsIndex {
    module: usize,
    offset: usize,
}

/// ELF's TLS resolver entry is not an ordinary C call boundary.
///
/// GNU-generated TLS sequences may temporarily reserve stack argument space
/// before calling `__tls_get_addr`, so the incoming RSP is not guaranteed to
/// have the alignment Rust assumes for an `extern "sysv64"` function.  The
/// real glibc resolver is assembly for the same reason.  Preserve the exact
/// guest frame, align a temporary host frame and provide Win64 shadow space
/// before entering Rust code.
#[unsafe(naked)]
unsafe extern "sysv64" fn elf_tls_get_addr(_index: *const TlsIndex) -> *mut u8 {
    core::arch::naked_asm!(
        "push rbp",
        "mov rbp, rsp",
        "and rsp, -16",
        "sub rsp, 32",
        "mov rcx, rdi",
        "call {implementation}",
        "mov rsp, rbp",
        "pop rbp",
        "ret",
        implementation = sym elf_tls_get_addr_aligned,
    )
}

unsafe extern "win64" fn elf_tls_get_addr_aligned(index: *const TlsIndex) -> *mut u8 {
    if index.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the ABI guarantees a pointer to a two-word tls_index here.
    let index = unsafe { &*index };
    kinakaze_tls::elf_tls_get_addr(index.module, index.offset).unwrap_or(ptr::null_mut())
}

unsafe extern "sysv64" fn elf_host_write(fd: i32, buffer: *const u8, count: usize) -> isize {
    if buffer.is_null() && count != 0 {
        return -(kinakaze_vfs::EFAULT as isize);
    }
    // SAFETY: the caller promises a readable range when `count` is nonzero. An
    // empty slice must not be built from a null pointer, so it is special-cased.
    let bytes = if count == 0 {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(buffer, count) }
    };
    match kinakaze_vfs::write(fd, bytes) {
        Ok(written) => written as isize,
        Err(errno) => -(errno as isize),
    }
}

/// Loads and links an object, then enters it the way the kernel would.
///
/// This is the path a real program takes. `_start` from `crt1.o` reads
/// `argc`/`argv`/`envp` off the stack and calls `__libc_start_main`, so it can
/// only be entered by pointing `rsp` at a synthesized stack block and jumping.
/// It never returns: the guest leaves through `exit`.
pub fn exec(
    path: &Path,
    arguments: &[String],
    image: Option<kinakaze_link::ImmutableBytes>,
    exec_environment: Option<Vec<String>>,
) -> Result<std::convert::Infallible, LinkError> {
    super::trace_spawn_loader_phase("exec-entered");
    if let Some(directory) = std::env::var_os("KINAKAZE_NAMESPACE_TRACE_DIR") {
        let path = std::path::PathBuf::from(directory)
            .join(format!("exec-command-{}.log", std::process::id()));
        let _ = std::fs::write(path, format!("{arguments:?}"));
    }
    crate::execution::segment_patch::warmup_decoder();
    super::trace_spawn_loader_phase("decoder-warmed");

    let host_directory = host_directory()?;
    super::trace_spawn_loader_phase("providers-ready");
    let mut linker = Box::new(Linker::new(
        SearchPaths::with_host_directory(host_directory).with_process_namespace(),
    ));
    super::trace_spawn_loader_phase("linker-allocated");
    linker.register_provider_registry(super::configuration()?.providers.clone())?;
    register_builtins(&mut linker)?;
    super::trace_spawn_loader_phase("linker-created");
    // Ordinary V2 launch keeps the linker's strict default: every required
    // strong import must resolve before any guest constructor or entry runs.

    // The guest reads its stack canary from `%fs:0x28`, which is address 0x28 on
    // Windows. Those reads are rewritten to a TEB slot, so the slot has to exist
    // and hold a canary before any guest code runs.
    let canary = canary_value() as usize;
    let slot = reserve_thread_pointer_slot(canary)?;
    let thread_pointer_slot = reserve_thread_pointer_slot(0)?;
    let host_transition_slot = reserve_thread_pointer_slot(0)?;
    let scratch_slot = reserve_thread_pointer_slot(0)?;
    if !kinakaze_runtime::register_fork_tls_slots(&[
        slot,
        thread_pointer_slot,
        host_transition_slot,
        scratch_slot,
    ]) {
        return Err(LinkError::InvalidTls {
            object: "loader TEB slots cannot be registered for fork".to_owned(),
        });
    }
    kinakaze_tls::configure_static_tls_teb_slot(thread_pointer_slot)
        .and_then(|()| kinakaze_tls::configure_host_transition_teb_slot(host_transition_slot))
        .and_then(|()| kinakaze_tls::configure_canary_teb_slot(slot, canary))
        .map_err(|error| LinkError::InvalidTls {
            object: format!("loader TEB configuration failed: {error:?}"),
        })?;
    linker.set_thread_pointer_slots(
        slot,
        thread_pointer_slot,
        host_transition_slot,
        scratch_slot,
    );
    install_fault_reporter(
        crate::execution::segment_patch::teb_slot_offset(slot),
        crate::execution::segment_patch::teb_slot_offset(thread_pointer_slot),
        crate::execution::segment_patch::teb_slot_offset(host_transition_slot),
        crate::execution::segment_patch::teb_slot_offset(scratch_slot),
    );
    super::trace_spawn_loader_phase("tls-and-veh-ready");

    let root = linker.load_process_graph(path, image)?;
    // ET_DYN covers both PIE executables and DSOs, so the process-entry path
    // must identify the former explicitly instead of guessing from ELF type.
    linker.mark_executable(root)?;
    super::trace_spawn_loader_phase("root-loaded");
    publish_active_linker(&mut linker)?;
    if super::report_traps_enabled() {
        eprintln!(
            "kinakaze: rewrote {} canary reads; nopped {} raw syscalls; {} other %fs: sites use guest TP",
            linker.patched_canary_sites(),
            linker.patched_syscall_sites(),
            linker.unpatched_thread_pointer_sites()
        );
    }
    let entry = linker.entry(root)?;
    super::install_requested_guest_breakpoint()?;
    let mut auxiliary = linker.auxiliary_values(root)?;
    if std::env::var_os("KINAKAZE_SPAWN_TRACE").is_some() {
        eprintln!(
            "kinakaze loader: aux phdr={:?} phent={} phnum={} entry={:?} base={:?} pagesz={}",
            auxiliary.program_headers,
            auxiliary.program_header_size,
            auxiliary.program_header_count,
            auxiliary.entry,
            auxiliary.interpreter_base,
            auxiliary.page_size,
        );
    }
    // AT_EXECFN is a guest-visible Linux path. The mapped object's retained
    // path is the canonical Windows provider path, which makes consumers
    // such as CPython parse `E:/...` as a program named merely `E`.
    auxiliary.executable_path = arguments.first().cloned();

    if super::report_traps_enabled() {
        let trapped = kinakaze_link::trap::trapped_symbols();
        eprintln!("kinakaze: {} symbols bound to traps", trapped.len());
        for name in &trapped {
            eprintln!("  trap: {name}");
        }
    }

    let environment: Vec<String> = if let Some(exec_env) = exec_environment {
        exec_env
    } else {
        let mut environment: Vec<String> = Vec::new();
        let mut env_keys = std::collections::HashSet::new();

        // Explicit host-to-guest overrides. Inheriting the complete Windows
        // environment would leak host path syntax (most notably PATH and
        // HOME) into Linux programs, while a fixed allow-list makes every
        // new runtime knob require loader changes. A prefixed opt-in bridge
        // keeps the boundary predictable and works for arbitrary guests:
        // KINAKAZE_GUEST_ENV_FOO=bar publishes FOO=bar to the guest.
        for (host_key, value) in std::env::vars_os() {
            let host_key = host_key.to_string_lossy();
            let Some(guest_key) = host_key.strip_prefix("KINAKAZE_GUEST_ENV_") else {
                continue;
            };
            let valid_key = !guest_key.is_empty()
                && guest_key.bytes().enumerate().all(|(index, byte)| {
                    byte == b'_'
                        || byte.is_ascii_alphabetic()
                        || (index != 0 && byte.is_ascii_digit())
                });
            if valid_key && env_keys.insert(guest_key.to_string()) {
                environment.push(format!("{guest_key}={}", value.to_string_lossy()));
            }
        }

        // 1. If /etc/environment exists or has synthetic defaults, load baseline variables
        if let Ok(path) = kinakaze_vfs::resolve_linux_path("/etc/environment")
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = trimmed.split_once('=') {
                    let k = k.trim();
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    if !k.is_empty() && env_keys.insert(k.to_string()) {
                        environment.push(format!("{k}={v}"));
                    }
                }
            }
        }

        // 2. Ensure essential POSIX defaults exist
        for (default_k, default_v) in [
            (
                "PATH",
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            ),
            ("HOME", "/root"),
            ("USER", "root"),
            ("LOGNAME", "root"),
            ("SHELL", "/bin/sh"),
            ("TERM", "xterm-256color"),
            ("LANG", "C.UTF-8"),
            ("DISPLAY", ":0"),
        ] {
            if env_keys.insert(default_k.to_string()) {
                environment.push(format!("{default_k}={default_v}"));
            }
        }

        // The initial host launch has no guest parent to supply shell
        // bookkeeping. Publish the actual VFS cwd as the default PWD while
        // still allowing an explicit KINAKAZE_GUEST_ENV_PWD override. CWD
        // remains process state; no filesystem operation trusts this value.
        if env_keys.insert("PWD".to_owned()) {
            environment.push(format!("PWD={}", kinakaze_vfs::fs::getcwd()));
        }

        environment
    };
    if std::env::var_os("KINAKAZE_ENV_TRACE").is_some() {
        let value_of = |wanted: &str| {
            environment.iter().find_map(|entry| {
                let (key, value) = entry.split_once('=')?;
                (key == wanted).then_some(value)
            })
        };
        eprintln!(
            "kinakaze: [TRACE guest-stack-init] logpipe={:?} loglevel={:?}",
            value_of("_LIBCONTAINER_LOGPIPE"),
            value_of("_LIBCONTAINER_LOGLEVEL")
        );
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
            "kinakaze: [TRACE guest-stack] environment count={} keys={keys}",
            environment.len()
        );
    }
    let image = kinakaze_link::stack::build(arguments, &environment, &auxiliary);
    let placed = kinakaze_link::launch::PlacedStack::place(image)?;
    libc::startup::publish_initial_stack(placed.stack_pointer());
    linker.link_all()?;
    super::trace_spawn_loader_phase("guest-stack-ready");
    if super::report_traps_enabled() {
        eprintln!(
            "kinakaze: guest stack region={:#x}..{:#x} image={:#x} entry-rsp={:#x}",
            placed.limit(),
            placed.limit() + 8 * 1024 * 1024,
            placed.base(),
            placed.stack_pointer()
        );
    }

    // Stack construction, diagnostics and VEH registration above are loader
    // work performed after linking, so they do not pass through an import
    // trampoline. Reactivate the already-built guest TP at this explicit
    // host-to-guest boundary instead of spending the first guest instruction
    // on a recoverable VEH fault.
    kinakaze_tls::install_current_thread_static_tls().map_err(|_| LinkError::InvalidTls {
        object: path.display().to_string(),
    })?;
    super::trace_spawn_loader_phase("guest-tls-ready");

    let argc = i32::try_from(arguments.len()).map_err(|_| LinkError::AddressOverflow)?;
    let argv = (placed.stack_pointer() + core::mem::size_of::<usize>()) as *const *const u8;
    // The stack builder places argv's null immediately after argc entries,
    // followed by envp. These pointers remain live for the process lifetime.
    let envp = unsafe { argv.add(arguments.len() + 1) };

    // The linker already crossed the manager barrier before IFUNC code.
    // Recheck activation at the explicit entry boundary as well, including
    // images without dependency constructors.
    kinakaze_runtime::authority::activate_image().map_err(|errno| {
        LinkError::InvalidProvider(format!("guest image activation failed: errno {errno}"))
    })?;

    // Linux startup has three distinct phases: executable preinit, dependency
    // constructors, then executable init from __libc_start_main. Only the
    // first two belong before the process entry point.
    // SAFETY: linking is complete and the vectors point into the placed stack.
    unsafe {
        linker.install_c_allocator()?;
        linker.run_executable_preinitializers(argc, argv, envp)?;
        linker.run_initializers_with_arguments(argc, argv, envp)?;
    }
    super::trace_spawn_loader_phase("guest-initializers-complete");

    // The TEB must describe the stack the guest will actually run on before any
    // guest code executes. Windows validates stack pointers against these bounds
    // while dispatching an exception, and a driver that raises and catches its
    // own C++ exception is enough to hit it — see `adopt_as_thread_stack`.
    // SAFETY: this is the thread that enters the guest, and the guest never
    // returns, so the original bounds are never needed again.
    unsafe { placed.adopt_as_thread_stack() };

    // `adopt_as_thread_stack` touches Windows' TEB and can cross a kernel
    // boundary. This is the final known host-to-guest edge; restoring here
    // keeps ordinary startup entirely off the VEH path.
    kinakaze_tls::install_current_thread_static_tls().map_err(|_| LinkError::InvalidTls {
        object: path.display().to_string(),
    })?;
    super::trace_spawn_loader_phase("entering-guest");

    // Destructors are deliberately not run here: control never comes back, and
    // the guest's own `atexit` chain handles teardown through `__libc_start_main`.
    // SAFETY: the image is fully linked and the stack block is live and leaked
    // for the process lifetime.
    if super::report_traps_enabled() {
        eprintln!(
            "ELF-LOADER: entering guest entry={:#x} rsp={:#x}",
            entry,
            placed.stack_pointer()
        );
    }
    unsafe { kinakaze_link::launch::enter(entry, placed.stack_pointer()) }
}

/// Reserves a TEB slot and seeds it with a stack-protector canary.
///
/// A Windows TLS slot is used rather than the `fs` base because Windows restores
/// `fs` from its own thread context on every context switch, discarding anything
/// `wrfsbase` wrote. The TEB is preserved per thread by construction, which is
/// what makes it the durable place to keep this.
///
/// Every thread that runs guest code needs its own value in the slot; this seeds
/// the thread that enters the guest.
fn reserve_thread_pointer_slot(value: usize) -> Result<u32, LinkError> {
    use windows_sys::Win32::System::Threading::{TlsAlloc, TlsSetValue};

    // SAFETY: TlsAlloc has no preconditions.
    let slot = unsafe { TlsAlloc() };
    if slot >= 64 {
        return Err(LinkError::InvalidTls {
            object: "no direct TEB TLS slot is available".to_owned(),
        });
    }
    // SAFETY: the slot was just allocated to this process.
    if unsafe { TlsSetValue(slot, value as *mut core::ffi::c_void) } == 0 {
        return Err(LinkError::InvalidTls {
            object: "TlsSetValue failed".to_owned(),
        });
    }
    Ok(slot)
}

/// A stack-protector canary.
///
/// Not cryptographic, and deliberately so: the canary defends against a buffer
/// overflow overwriting a return address, for which any value not predictable
/// from the binary suffices. The low byte is zero because a string overflow stops
/// at a NUL, so keeping one there prevents byte-at-a-time discovery — that is the
/// same reason glibc does it.
fn canary_value() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::from(elapsed.subsec_nanos()))
        .unwrap_or(0);
    let local = 0u64;
    let value = nanos.rotate_left(17)
        ^ u64::from(std::process::id()).rotate_left(31)
        ^ std::ptr::from_ref(&local) as u64;
    let value = value & !0xff;
    if value == 0 {
        0x0123_4567_89ab_cd00
    } else {
        value
    }
}

fn host_directory() -> Result<PathBuf, LinkError> {
    Ok(super::configuration()?.root.clone())
}

/// Restore an explicit exec handoff using the same VFS instance as syscalls.
pub fn consume_exec_handoff() -> Result<(), LinkError> {
    let initialized = libc::kinakaze_process_consume_exec_handoff();
    kinakaze_vfs::release_retained_exec_handoff();
    if initialized != 1 {
        return Err(LinkError::InvalidProvider(format!(
            "exec handoff initialization returned {initialized}"
        )));
    }
    Ok(())
}

pub fn publish_loader_exit(status: i32) {
    libc::kinakaze_process_publish_loader_exit(status);
}

/// Loads, links, initializes and runs an object, then runs its destructors.
pub fn run(path: &Path, image: Option<kinakaze_link::ImmutableBytes>) -> Result<isize, LinkError> {
    // Pre-initialize iced-x86 decoder tables so VEH handler doesn't allocate lazily
    crate::execution::segment_patch::warmup_decoder();

    let mut linker = Box::new(Linker::new(
        SearchPaths::with_host_directory(host_directory()?).with_process_namespace(),
    ));
    linker.register_provider_registry(super::configuration()?.providers.clone())?;
    register_builtins(&mut linker)?;

    let root = match image {
        Some(image) => linker.load_image(path, image)?,
        None => linker.load(path)?,
    };
    publish_active_linker(&mut linker)?;
    let entry = linker.entry(root)?;

    kinakaze_runtime::authority::activate_image().map_err(|errno| {
        LinkError::InvalidProvider(format!("guest image activation failed: errno {errno}"))
    })?;

    // Initializers are guest code and must not run until the whole graph is
    // linked, which `load` has completed by this point.
    // SAFETY: linking finished above.
    unsafe {
        linker.install_c_allocator()?;
        linker.run_initializers()?;
    }

    // SAFETY: the parser validated x86_64 and the entry lies inside an
    // executable segment of a fully relocated image.
    let entry: unsafe extern "sysv64" fn() -> isize = unsafe { std::mem::transmute(entry) };
    // SAFETY: the guest's `_start` has this no-argument ABI.
    let status = unsafe { entry() };

    // Destructors run in reverse dependency order while the image is intact.
    // SAFETY: nothing has been unmapped yet.
    unsafe { kinakaze_link::process::run_finalizers()? };
    clear_active_linker();
    Ok(status)
}
