//! Symbols glibc's own startup path calls before `main`.
//!
//! These are not things a program asks for; they are what `__libc_start_main` and
//! the allocator's initialization touch on the way in. A real binary therefore
//! cannot reach `main` without them, which is what makes them the first things a
//! hosted glibc-linked executable needs.

#[cfg(all(windows, target_arch = "x86_64"))]
pub use windows::*;

/// Initial guest stack pointer, also visible through an executable's COPY
/// relocation. Published by the loader before any guest initializer runs.
#[unsafe(no_mangle)]
pub static kinakaze_abi___libc_stack_end: crate::copied::CopiedPointer =
    crate::copied::CopiedPointer::new();

pub fn publish_initial_stack(stack_pointer: usize) {
    kinakaze_abi___libc_stack_end.set(stack_pointer as *mut core::ffi::c_char);
}

#[cfg(all(windows, target_arch = "x86_64"))]
mod windows {
    use core::ffi::{c_char, c_int, c_void};

    /// `mallopt`: tunes allocator behaviour.
    ///
    /// glibc's startup calls this, and BusyBox calls it directly to set
    /// `M_TRIM_THRESHOLD`. Every parameter it can set is advice about an
    /// implementation detail of glibc's allocator — arena counts, trim and mmap
    /// thresholds — none of which exist in the managed heap behind this libc.
    ///
    /// Returning success is truthful in the only sense the caller can observe: the
    /// request was accepted, and the allocator is free to ignore advice. Returning
    /// failure would be worse, since some callers treat it as a fatal
    /// misconfiguration.
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_mallopt(_parameter: c_int, _value: c_int) -> c_int {
        // glibc returns 1 for success here, not 0.
        1
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn mallopt(parameter: c_int, value: c_int) -> c_int {
        kinakaze_abi_mallopt(parameter, value)
    }

    /// `mtrace` / `muntrace`: allocation-debugging compatibility entry points.
    ///
    /// Since glibc 2.34 the implementation lives in `libc_malloc_debug.so` and
    /// the symbols exported by libc itself are deliberately inert unless that
    /// debugging DSO interposes them. This provider does not claim that the DSO
    /// was preloaded, so matching libc's own entry points means doing nothing;
    /// in particular, merely setting `MALLOC_TRACE` must not add allocator work
    /// or create a trace file in a normal process.
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_mtrace() {}

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn mtrace() {
        kinakaze_abi_mtrace()
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_muntrace() {}

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn muntrace() {
        kinakaze_abi_muntrace()
    }

    /// `__libc_current_sigrtmin` / `__libc_current_sigrtmax`: the realtime signal
    /// range.
    ///
    /// Linux reserves 32..=64, with glibc taking the first few for its own use.
    /// Reporting the standard range keeps a caller that iterates it from walking
    /// off into signal numbers this layer does not know.
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi___libc_current_sigrtmin() -> c_int {
        34
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn __libc_current_sigrtmin() -> c_int {
        34
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi___libc_current_sigrtmax() -> c_int {
        64
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn __libc_current_sigrtmax() -> c_int {
        64
    }

    /// `__h_errno_location`: the resolver's error variable.
    ///
    /// Separate from `errno` because a name lookup failure is not an OS error. It is
    /// thread-local for the same reason `errno` is: two threads resolving names
    /// concurrently must not overwrite each other's failure.
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi___h_errno_location() -> *mut c_int {
        thread_local! {
            static H_ERRNO: core::cell::UnsafeCell<c_int> = const {
                core::cell::UnsafeCell::new(0)
            };
        }
        H_ERRNO.with(|slot| slot.get())
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn __h_errno_location() -> *mut c_int {
        kinakaze_abi___h_errno_location()
    }

    /// `__ctype_b_loc`: the character-class table `isalpha` and friends index.
    ///
    /// The table is indexed from `-128` to `255`, so the returned pointer is
    /// deliberately offset into the middle of the storage: glibc's macros do
    /// `(*__ctype_b_loc())[c]` with a possibly negative `c`, and a pointer to the
    /// start would read out of bounds for every high-bit byte.
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi___ctype_b_loc() -> *mut *const u16 {
        thread_local! {
            static POINTER: core::cell::UnsafeCell<*const u16> = const {
                core::cell::UnsafeCell::new(core::ptr::null())
            };
        }
        POINTER.with(|slot| {
            // SAFETY: single-threaded access to this thread's own cell.
            unsafe {
                if (*slot.get()).is_null() {
                    *slot.get() = ctype_table().as_ptr().add(128);
                }
            }
            slot.get()
        })
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn __ctype_b_loc() -> *mut *const u16 {
        kinakaze_abi___ctype_b_loc()
    }

    // The bit flags glibc's `ctype` macros test, in glibc's own order.
    const UPPER: u16 = 1 << 8;
    const LOWER: u16 = 1 << 9;
    const ALPHA: u16 = 1 << 10;
    const DIGIT: u16 = 1 << 11;
    const XDIGIT: u16 = 1 << 12;
    const SPACE: u16 = 1 << 13;
    const PRINT: u16 = 1 << 14;
    const GRAPH: u16 = 1 << 15;
    const BLANK: u16 = 1 << 0;
    const CNTRL: u16 = 1 << 1;
    const PUNCT: u16 = 1 << 2;
    const ALNUM: u16 = 1 << 3;

    /// Builds the 384-entry class table for the C locale.
    ///
    /// Only the C locale is described. That is a real limitation rather than an
    /// oversight: honouring `LC_CTYPE` would mean carrying locale data, and
    /// pretending to while returning ASCII classifications would be worse than
    /// being clear that this is the C locale.
    fn ctype_table() -> &'static [u16; 384] {
        use std::sync::OnceLock;
        static TABLE: OnceLock<[u16; 384]> = OnceLock::new();
        TABLE.get_or_init(|| {
            let mut table = [0u16; 384];
            for index in 0..256usize {
                let byte = index as u8;
                let mut flags = 0u16;
                if byte.is_ascii_uppercase() {
                    flags |= UPPER | ALPHA | ALNUM | PRINT | GRAPH;
                }
                if byte.is_ascii_lowercase() {
                    flags |= LOWER | ALPHA | ALNUM | PRINT | GRAPH;
                }
                if byte.is_ascii_digit() {
                    flags |= DIGIT | ALNUM | PRINT | GRAPH | XDIGIT;
                }
                if byte.is_ascii_hexdigit() {
                    flags |= XDIGIT;
                }
                if matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') {
                    flags |= SPACE;
                }
                if matches!(byte, b' ' | b'\t') {
                    flags |= BLANK;
                }
                if byte.is_ascii_control() || byte == 0x7f {
                    flags |= CNTRL;
                }
                if byte.is_ascii_punctuation() {
                    flags |= PUNCT | PRINT | GRAPH;
                }
                if byte == b' ' {
                    flags |= PRINT;
                }
                // Index 128 is the zero point: entries below it describe the
                // negative half of a signed char.
                table[index + 128] = flags;
            }
            table
        })
    }

    /// `__ctype_tolower_loc` and `__ctype_toupper_loc`: case-mapping tables.
    ///
    /// Same negative-index convention as [`kinakaze_abi___ctype_b_loc`].
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi___ctype_tolower_loc() -> *mut *const c_int {
        case_table(false)
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn __ctype_tolower_loc() -> *mut *const c_int {
        case_table(false)
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi___ctype_toupper_loc() -> *mut *const c_int {
        case_table(true)
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn __ctype_toupper_loc() -> *mut *const c_int {
        case_table(true)
    }

    fn case_table(upper: bool) -> *mut *const c_int {
        use std::sync::OnceLock;
        static LOWER_TABLE: OnceLock<[c_int; 384]> = OnceLock::new();
        static UPPER_TABLE: OnceLock<[c_int; 384]> = OnceLock::new();
        let build = |upper: bool| {
            let mut table = [0 as c_int; 384];
            for index in 0..256usize {
                let byte = index as u8;
                let mapped = if upper {
                    byte.to_ascii_uppercase()
                } else {
                    byte.to_ascii_lowercase()
                };
                table[index + 128] = c_int::from(mapped);
            }
            table
        };
        let table = if upper {
            UPPER_TABLE.get_or_init(|| build(true))
        } else {
            LOWER_TABLE.get_or_init(|| build(false))
        };

        thread_local! {
            static LOWER_POINTER: core::cell::UnsafeCell<*const c_int> = const {
                core::cell::UnsafeCell::new(core::ptr::null())
            };
            static UPPER_POINTER: core::cell::UnsafeCell<*const c_int> = const {
                core::cell::UnsafeCell::new(core::ptr::null())
            };
        }
        let slot = if upper {
            &UPPER_POINTER
        } else {
            &LOWER_POINTER
        };
        slot.with(|cell| {
            // SAFETY: this thread's own cell, initialized once.
            unsafe { *cell.get() = table.as_ptr().add(128) };
            cell.get()
        })
    }

    /// `__assert_fail`: reports a failed assertion and aborts.
    ///
    /// The wording matches glibc's so a caller comparing output is not surprised.
    ///
    /// # Safety
    ///
    /// Every pointer must be null or a valid NUL-terminated string.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___assert_fail(
        assertion: *const c_char,
        file: *const c_char,
        line: c_int,
        function: *const c_char,
    ) -> ! {
        // SAFETY: forwarded from this function's contract.
        let text = |pointer: *const c_char| unsafe {
            if pointer.is_null() {
                "<null>".to_owned()
            } else {
                core::ffi::CStr::from_ptr(pointer)
                    .to_string_lossy()
                    .into_owned()
            }
        };
        eprintln!(
            "kinakaze: {}:{}: {}: Assertion `{}' failed.",
            text(file),
            line,
            text(function),
            text(assertion)
        );
        std::process::abort()
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __assert_fail(
        assertion: *const c_char,
        file: *const c_char,
        line: c_int,
        function: *const c_char,
    ) -> ! {
        unsafe { kinakaze_abi___assert_fail(assertion, file, line, function) }
    }

    /// `__stack_chk_fail`: the stack-protector's failure path.
    ///
    /// Reached only when a canary check found the frame corrupted, so continuing is
    /// never an option: the whole point is to stop before the corrupted return
    /// address is used.
    ///
    /// The failing frame is still on the stack when this runs, which makes it the
    /// only place the corruption can be reported from — `abort` discards every
    /// buffered diagnostic.
    ///
    /// A Windows backtrace cannot help here: it unwinds with unwind data the
    /// guest's mapped code does not have, so the walk stops at this frame and
    /// never reaches the function that failed. The return address is still
    /// sitting at `[rsp]` though, and it points into the failing function, so a
    /// naked shim reads it before any prologue can move it. `llvm-objdump`
    /// resolves the address once the object's load base is known.
    #[unsafe(no_mangle)]
    #[unsafe(naked)]
    pub extern "sysv64" fn kinakaze_abi___stack_chk_fail() -> ! {
        core::arch::naked_asm!(
            "mov rdi, [rsp]",  // SysV argument 0: our return address.
            "mov rsi, rsp",    // argument 1: the stack pointer, for context.
            "sub rsp, 8",      // Realign to 16 before the call, per the SysV ABI.
            "call {report}",
            "ud2",             // `report` does not return; never fall through.
            report = sym report_stack_smashing,
        )
    }

    #[unsafe(no_mangle)]
    #[unsafe(naked)]
    pub extern "sysv64" fn __stack_chk_fail() -> ! {
        core::arch::naked_asm!(
            "mov rdi, [rsp]",
            "mov rsi, rsp",
            "sub rsp, 8",
            "call {report}",
            "ud2",
            report = sym report_stack_smashing,
        )
    }

    /// Reports the frame that failed its canary check and stops the process.
    ///
    /// The absolute address is useless on its own — a PIE guest is mapped at a
    /// different base every run, which is exactly why the bare address looks
    /// random. `VirtualQuery` turns it into an offset from the region it lives
    /// in, and *that* is stable and resolvable with `llvm-objdump`.
    extern "sysv64" fn report_stack_smashing(return_address: usize, stack_pointer: usize) -> ! {
        use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};

        eprintln!("kinakaze: *** stack smashing detected ***: terminated");

        let mut info = core::mem::MaybeUninit::<MEMORY_BASIC_INFORMATION>::uninit();
        let written = unsafe {
            VirtualQuery(
                return_address as *const core::ffi::c_void,
                info.as_mut_ptr(),
                core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        let mut code_region = None;
        if written != 0 {
            // SAFETY: `VirtualQuery` reported success, so the structure is filled in.
            let info = unsafe { info.assume_init() };
            let base = info.AllocationBase as usize;
            eprintln!(
                "kinakaze: failing check at {return_address:#x} = base {base:#x} + {:#x}",
                return_address.wrapping_sub(base)
            );
            code_region = Some((info.BaseAddress as usize, info.RegionSize));
        } else {
            eprintln!("kinakaze: failing check at {return_address:#x}");
        }

        // Addresses above the return address are the failing function's frame,
        // and one of them is usually its caller.
        let window = unsafe { core::slice::from_raw_parts(stack_pointer as *const usize, 12) };
        for (index, slot) in window.iter().enumerate() {
            eprintln!("kinakaze:   stack[{index}] = {slot:#x}");
        }

        // A stack-protected function reads the guard in its prologue and again
        // in its epilogue.  The ELF patcher must redirect both `%fs:0x28`
        // operands to the same TEB TLS slot.  Print every nearby guard access so
        // a missed/stale AOT patch is distinguishable from actual frame damage.
        if let Some((region_start, region_len)) = code_region {
            let region_end = region_start.saturating_add(region_len);
            let scan_start = return_address.saturating_sub(0x400).max(region_start);
            let scan_end = return_address.saturating_add(16).min(region_end);
            if scan_end >= scan_start.saturating_add(9) {
                let code = unsafe {
                    core::slice::from_raw_parts(scan_start as *const u8, scan_end - scan_start)
                };
                for index in 0..=code.len() - 9 {
                    let segment = code[index];
                    let operation = code[index + 2];
                    if matches!(segment, 0x64 | 0x65)
                        && code[index + 1] == 0x48
                        && matches!(operation, 0x8b | 0x2b)
                        && code[index + 3] == 0x04
                        && code[index + 4] == 0x25
                    {
                        let displacement =
                            u32::from_le_bytes(code[index + 5..index + 9].try_into().unwrap())
                                as usize;
                        let segment_name = if segment == 0x65 { "gs" } else { "fs" };
                        let operation_name = if operation == 0x8b { "load" } else { "compare" };
                        let address = scan_start + index;
                        eprintln!(
                            "kinakaze:   guard {operation_name} at {address:#x}: {segment_name}:[{displacement:#x}]"
                        );
                        if segment == 0x65 {
                            let teb: usize;
                            unsafe {
                                core::arch::asm!(
                                    "mov {}, gs:[0x30]",
                                    out(reg) teb,
                                    options(nostack, preserves_flags, readonly)
                                );
                                let current = *((teb + displacement) as *const usize);
                                eprintln!("kinakaze:     current guard value = {current:#x}");
                            }
                        }
                    }
                }
            }
        }
        std::process::abort()
    }

    /// `__chk_fail`: the fortified functions' failure path.
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi___chk_fail() -> ! {
        eprintln!("kinakaze: *** buffer overflow detected ***: terminated");
        // Fortify failures belong to the guest process: deliver SIGABRT and
        // publish signal termination so its parent can reap it normally.
        let _ = kinakaze_vfs::signal::sigprocmask(kinakaze_vfs::signal::SIG_UNBLOCK, 1 << 5);
        crate::signal::kinakaze_abi_raise(6);
        crate::process::terminate_from_signal(6);
        unreachable!()
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn __chk_fail() -> ! {
        kinakaze_abi___chk_fail()
    }

    /// `__libc_start_main`: the handoff from `_start` to `main`.
    ///
    /// This is the pivot of hosting a real program. `crt1.o`'s `_start` reads
    /// `argc`/`argv` off the stack the loader built, then calls this with `main`
    /// and the constructor/destructor hooks. Everything a C program assumes is
    /// true on entry to `main` is arranged here.
    ///
    /// It never returns: `main`'s value goes to `exit`, so the process leaves
    /// through the normal teardown path with `atexit` handlers and stream flushes
    /// intact. Returning instead would drop back into `_start`, which has nothing
    /// to return to.
    ///
    /// # Safety
    ///
    /// `main` must be a valid entry point, `argv` must hold `argc` pointers
    /// followed by a null, and the function-pointer arguments must be null or
    /// callable.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___libc_start_main(
        main: Option<unsafe extern "sysv64" fn(c_int, *mut *mut c_char, *mut *mut c_char) -> c_int>,
        argc: c_int,
        argv: *mut *mut c_char,
        init: Option<unsafe extern "sysv64" fn(c_int, *mut *mut c_char, *mut *mut c_char)>,
        fini: Option<unsafe extern "sysv64" fn()>,
        rtld_fini: Option<unsafe extern "sysv64" fn()>,
        _stack_end: *mut c_void,
    ) -> ! {
        // The environment sits immediately after argv's null terminator. That
        // adjacency is the ABI: there is no separate pointer to it, which is why
        // argv must be null-terminated for envp to be findable at all.
        let environment = if argv.is_null() {
            core::ptr::null_mut()
        } else {
            // SAFETY: the caller guarantees `argc` pointers plus a null.
            unsafe { argv.add(argc.max(0) as usize + 1) }
        };

        // Publish it before anything else runs: a constructor may call `getenv`,
        // and it has to see the real environment rather than a copy of the host's.
        if !environment.is_null() {
            crate::process::adopt(environment);
        }
        if argc > 0 && !argv.is_null() {
            // SAFETY: the caller guarantees `argc` live argv pointers.
            crate::misc::set_program_invocation(unsafe { *argv });
        }

        // The dynamic linker's finalizer runs last, so it is registered first.
        if let Some(rtld_fini) = rtld_fini {
            register_teardown(rtld_fini);
        }
        if let Some(fini) = fini {
            register_teardown(fini);
        }

        // Constructors run before `main`, after the environment is in place.
        // Older crt objects pass __libc_csu_init directly. Current glibc crt
        // passes null and expects libc to locate the executable's dynamic entries
        // through the process link-map, which is owned by the host loader here.
        match init {
            Some(init) => {
                // SAFETY: the caller guarantees `init` is callable with this ABI.
                unsafe { init(argc, argv, environment) };
            }
            None => {
                // SAFETY: argv/envp are the live process vectors guaranteed by
                // this function's ABI.
                unsafe { run_host_main_initializers(argc, argv, environment) };
            }
        }

        let status = match main {
            // SAFETY: the caller guarantees `main` is callable with this ABI.
            Some(main) => {
                if std::env::var_os("KINAKAZE_DEBUG_STARTUP").is_some()
                    || std::env::var_os("KINAKAZE_VERBOSE").is_some()
                {
                    eprintln!(
                        "kinakaze: [STARTUP] calling guest main(argc={}, argv={:?}, envp={:?})",
                        argc, argv, environment
                    );
                }
                let status = unsafe { main(argc, argv, environment) };
                if std::env::var_os("KINAKAZE_DEBUG_STARTUP").is_some()
                    || std::env::var_os("KINAKAZE_VERBOSE").is_some()
                {
                    eprintln!("kinakaze: [STARTUP] guest main returned {status}");
                }
                status
            }
            // A null `main` cannot be called, and there is nothing sensible to run
            // instead, so it is reported rather than silently treated as success.
            None => {
                eprintln!("kinakaze: __libc_start_main was given a null main");
                70
            }
        };

        crate::process::kinakaze_abi_exit(status)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __libc_start_main(
        main: Option<unsafe extern "sysv64" fn(c_int, *mut *mut c_char, *mut *mut c_char) -> c_int>,
        argc: c_int,
        argv: *mut *mut c_char,
        init: Option<unsafe extern "sysv64" fn(c_int, *mut *mut c_char, *mut *mut c_char)>,
        fini: Option<unsafe extern "sysv64" fn()>,
        rtld_fini: Option<unsafe extern "sysv64" fn()>,
        stack_end: *mut c_void,
    ) -> ! {
        unsafe {
            kinakaze_abi___libc_start_main(main, argc, argv, init, fini, rtld_fini, stack_end)
        }
    }

    /// Invokes the process loader's main-executable constructor phase.
    ///
    /// This is mandatory for modern dynamically linked startup. Continuing after
    /// a missing export or a loader failure would enter `main` without constructors
    /// and violate the ELF process ABI, so both cases terminate explicitly.
    unsafe fn run_host_main_initializers(
        argc: c_int,
        argv: *mut *mut c_char,
        envp: *mut *mut c_char,
    ) {
        let Some(run) = kinakaze_runtime::services::main_initializers() else {
            startup_loader_failure("loader services are not initialized");
        };
        // SAFETY: the process stack owns both vectors until process termination.
        if unsafe { run(argc, argv.cast(), envp.cast()) } != 0 {
            startup_loader_failure("main executable constructors failed");
        }
    }

    fn startup_loader_failure(reason: &str) -> ! {
        eprintln!("kinakaze: dynamic startup failed: {reason}");
        crate::process::terminate_host_process(127)
    }

    /// Registers a teardown hook with the `atexit` chain.
    ///
    /// `atexit` takes a no-argument function, which is exactly the shape of both
    /// `fini` and `rtld_fini`, so no adapter is needed.
    fn register_teardown(hook: unsafe extern "sysv64" fn()) {
        // SAFETY: the hook is a callable no-argument function, which is what
        // `atexit` requires.
        if unsafe { crate::process::kinakaze_abi_atexit(Some(hook)) } != 0 {
            startup_loader_failure("cannot register finalizer");
        }
    }

    /// `__libc_malloc` and friends: glibc's internal allocator names.
    ///
    /// Some code links against these directly rather than the public spellings.
    ///
    /// # Safety
    ///
    /// As the public allocator entry points.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___libc_malloc(size: usize) -> *mut c_void {
        // SAFETY: forwarded to the managed allocator with the caller's size.
        unsafe { crate::kinakaze_malloc(size) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __libc_malloc(size: usize) -> *mut c_void {
        unsafe { kinakaze_abi___libc_malloc(size) }
    }

    /// # Safety
    ///
    /// `pointer` must come from this allocator.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___libc_free(pointer: *mut c_void) {
        // SAFETY: forwarded from this function's contract.
        unsafe { crate::kinakaze_free(pointer) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __libc_free(pointer: *mut c_void) {
        unsafe { kinakaze_abi___libc_free(pointer) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___libc_calloc(
        count: usize,
        size: usize,
    ) -> *mut c_void {
        unsafe { crate::kinakaze_calloc(count, size) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___libc_realloc(
        pointer: *mut c_void,
        size: usize,
    ) -> *mut c_void {
        unsafe { crate::kinakaze_realloc(pointer, size) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___libc_memalign(
        alignment: usize,
        size: usize,
    ) -> *mut c_void {
        unsafe { crate::kinakaze_abi_memalign(alignment, size) }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_ctype_table_classifies_the_c_locale() {
            let table = ctype_table();
            // The pointer handed out is offset by 128, so index it the way glibc's
            // macros do.
            let at = |c: i32| table[(c + 128) as usize];

            assert_ne!(at(i32::from(b'A')) & ALPHA, 0);
            assert_ne!(at(i32::from(b'A')) & UPPER, 0);
            assert_eq!(at(i32::from(b'A')) & LOWER, 0);
            assert_ne!(at(i32::from(b'z')) & LOWER, 0);
            assert_ne!(at(i32::from(b'5')) & DIGIT, 0);
            assert_ne!(at(i32::from(b'5')) & XDIGIT, 0);
            assert_ne!(at(i32::from(b'f')) & XDIGIT, 0);
            assert_eq!(at(i32::from(b'g')) & XDIGIT, 0);
            assert_ne!(at(i32::from(b' ')) & SPACE, 0);
            assert_ne!(at(i32::from(b' ')) & BLANK, 0);
            assert_ne!(at(i32::from(b'\n')) & SPACE, 0);
            // A newline is whitespace but not blank: only space and tab are blank.
            assert_eq!(at(i32::from(b'\n')) & BLANK, 0);
            assert_ne!(at(i32::from(b',')) & PUNCT, 0);
            assert_ne!(at(0) & CNTRL, 0);
            // A letter is not punctuation, which would break isprint groupings.
            assert_eq!(at(i32::from(b'A')) & PUNCT, 0);
        }

        #[test]
        fn the_negative_half_of_the_table_is_addressable() {
            // glibc's macros pass a possibly-negative `char`, so the table has to
            // be readable below its zero point. Reading there must not be out of
            // bounds, which is the whole reason for the 128-entry prefix.
            let table = ctype_table();
            let negative = table[(-1i32 + 128) as usize];
            // Nothing is classified there; the point is only that it is readable.
            assert_eq!(negative, 0);
            assert_eq!(table.len(), 384);
        }

        #[test]
        fn case_tables_map_ascii_both_ways() {
            let lower = case_table(false);
            let upper = case_table(true);
            // SAFETY: both pointers were just produced by `case_table`.
            unsafe {
                let lower = *lower;
                let upper = *upper;
                assert_eq!(*lower.offset(i32::from(b'A') as isize), i32::from(b'a'));
                assert_eq!(*upper.offset(i32::from(b'a') as isize), i32::from(b'A'));
                // A digit maps to itself in both directions.
                assert_eq!(*lower.offset(i32::from(b'7') as isize), i32::from(b'7'));
                assert_eq!(*upper.offset(i32::from(b'7') as isize), i32::from(b'7'));
            }
        }

        #[test]
        fn mallopt_reports_success() {
            // glibc returns 1, not 0, and callers do check.
            assert_eq!(kinakaze_abi_mallopt(-1, 0), 1);
        }
    }
}
