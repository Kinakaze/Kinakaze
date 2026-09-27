//! Process exit callbacks; guest addresses survive fork, private owners do not.
mod lifecycle;
use core::ffi::{c_int, c_void};
use std::sync::{Mutex, OnceLock};

/// An `atexit` handler.
pub type ExitHandler = unsafe extern "sysv64" fn();

/// A `__cxa_atexit` handler, which receives the argument registered with it.
pub type CxaHandler = unsafe extern "sysv64" fn(*mut c_void);

/// One registered termination handler.
///
/// `atexit` and `__cxa_atexit` share a single list because C++ requires the two
/// to interleave: handlers run in reverse order of registration regardless of
/// which call registered them, which separate lists could not reproduce.
#[derive(Clone, Copy)]
pub(super) enum Handler {
    /// Registered through `atexit`.
    Plain(ExitHandler),
    Quick(ExitHandler),
    QuickCxa {
        function: CxaHandler,
        dso: *mut c_void,
    },
    /// Registered through `__cxa_atexit`, tagged with its owning object.
    Cxa {
        function: CxaHandler,
        argument: *mut c_void,
        dso: *mut c_void,
    },
}

// SAFETY: the pointers are opaque tokens here. The argument is only ever handed
// back to the handler that registered it and the DSO handle is compared, never
// dereferenced.
unsafe impl Send for Handler {}

/// Registered handlers, run in reverse order of registration.
static EXIT_HANDLERS: OnceLock<Mutex<Vec<Handler>>> = OnceLock::new();

pub(super) fn exit_handlers() -> &'static Mutex<Vec<Handler>> {
    EXIT_HANDLERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn trace_exit_stage(stage: &str, callback: usize) {
    let Some(directory) = std::env::var_os("KINAKAZE_NAMESPACE_TRACE_DIR") else {
        return;
    };
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(
        std::path::PathBuf::from(directory).join(format!("exit-stages-{}.log", std::process::id())),
    ) {
        let _ = writeln!(file, "{stage} callback={callback:#x}");
    }
}

/// Invokes one handler.
///
/// # Safety
///
/// The registrar promised the function stays callable until the process exits.
unsafe fn run(handler: Handler) {
    let callback = match handler {
        Handler::Plain(function) | Handler::Quick(function) => function as usize,
        Handler::QuickCxa { function, .. } | Handler::Cxa { function, .. } => function as usize,
    };
    trace_exit_stage("callback-enter", callback);
    match handler {
        // SAFETY: forwarded from this function's contract.
        Handler::Plain(function) | Handler::Quick(function) => unsafe { function() },
        Handler::QuickCxa { function, .. } => unsafe { function(core::ptr::null_mut()) },
        Handler::Cxa {
            function, argument, ..
        } => {
            // SAFETY: forwarded from this function's contract. The argument is
            // the one supplied at registration.
            unsafe { function(argument) }
        }
    }
    trace_exit_stage("callback-return", callback);
}

/// Registers a handler, reporting failure the way `atexit` does.
fn register(handler: Handler) -> c_int {
    match exit_handlers().lock() {
        Ok(mut handlers) => {
            if handlers.try_reserve(1).is_err() {
                return -1;
            }
            handlers.push(handler);
            0
        }
        Err(_) => -1,
    }
}

/// `atexit`.
///
/// # Safety
///
/// `handler` must remain callable until the process exits.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_atexit(handler: Option<ExitHandler>) -> c_int {
    let Some(handler) = handler else {
        return -1;
    };
    register(Handler::Plain(handler))
}

/// Registers a C11 quick-exit callback separately from normal termination.
/// # Safety
/// The callback must remain callable until quick_exit.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_at_quick_exit(handler: Option<ExitHandler>) -> c_int {
    handler.map_or(-1, |handler| register(Handler::Quick(handler)))
}

/// GNU C++ quick-exit ABI: the callback receives a null argument.
/// # Safety
/// The callback must remain callable until quick_exit or DSO finalization.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___cxa_at_quick_exit(
    function: Option<CxaHandler>,
    dso: *mut c_void,
) -> c_int {
    function.map_or(-1, |function| register(Handler::QuickCxa { function, dso }))
}

fn run_handlers(quick: bool) {
    loop {
        let next = exit_handlers().lock().ok().and_then(|mut handlers| {
            handlers
                .iter()
                .rposition(|handler| {
                    matches!(handler, Handler::Quick(_) | Handler::QuickCxa { .. }) == quick
                })
                .map(|index| handlers.remove(index))
        });
        let Some(handler) = next else { break };
        // SAFETY: validity was promised when the callback was registered.
        unsafe { run(handler) };
    }
}

/// C11 quick exit skips normal handlers, TLS destructors and stream flushing.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_quick_exit(status: c_int) -> ! {
    run_handlers(true);
    super::kinakaze_abi__exit(status)
}

/// `__cxa_atexit`, the C++ form that passes an argument to the handler.
///
/// # Safety
///
/// `function` must stay callable, and `argument` valid, until the handler runs.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___cxa_atexit(
    function: Option<CxaHandler>,
    argument: *mut c_void,
    dso: *mut c_void,
) -> c_int {
    let Some(function) = function else {
        return -1;
    };
    register(Handler::Cxa {
        function,
        argument,
        dso,
    })
}

/// `__cxa_finalize`, which runs and removes the handlers of one shared object.
///
/// Called from a library's destructor so its handlers do not outlive the code
/// they point into. A null `dso` runs every `__cxa_atexit` handler, which is what
/// happens on process exit. Handlers registered through `atexit` are left alone:
/// they belong to the program, not to any one object.
///
/// # Safety
///
/// The handlers being run must still be callable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___cxa_finalize(dso: *mut c_void) {
    // Reverse registration order, and each handler is removed before it runs so
    // a handler that triggers another finalize cannot run it twice.
    loop {
        let next = match exit_handlers().lock() {
            Ok(mut handlers) => {
                let found = handlers.iter().rposition(|handler| match handler {
                    Handler::Cxa { dso: owner, .. } => dso.is_null() || *owner == dso,
                    Handler::Plain(_) | Handler::Quick(_) | Handler::QuickCxa { .. } => false,
                });
                found.map(|index| handlers.remove(index))
            }
            Err(_) => None,
        };
        match next {
            // SAFETY: forwarded from this function's contract.
            Some(handler) => unsafe { run(handler) },
            None => break,
        }
    }
    if !dso.is_null() {
        if let Ok(mut handlers) = exit_handlers().lock() {
            handlers.retain(
                |handler| !matches!(handler, Handler::QuickCxa { dso: owner, .. } if *owner == dso),
            );
        }
    }
}

/// Runs `atexit` handlers and flushes streams, then terminates.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_exit(status: c_int) -> ! {
    trace_exit_stage("exit-enter", status as usize);
    // The initial thread's C++ thread_local objects precede process atexit
    // handlers. Pthread key destructors belong to thread exit and do not run
    // as part of this process-exit path.
    kinakaze_tls::run_cxx_thread_destructors();
    trace_exit_stage("tls-destructors-return", 0);
    // C requires handlers to run in reverse registration order. The lock is
    // released before each call so a handler may itself call `atexit`.
    run_handlers(false);
    // Buffered output must reach the descriptors before the process dies.
    let _ = crate::stdio::flush_all();
    trace_exit_stage("stdio-flushed", 0);
    super::kinakaze_abi__exit(status)
}

#[cfg(test)]
mod quick_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ORDER: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "sysv64" fn normal() {
        ORDER
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n * 10 + 1))
            .unwrap();
    }
    unsafe extern "sysv64" fn quick() {
        ORDER
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n * 10 + 2))
            .unwrap();
    }
    unsafe extern "sysv64" fn cxa(argument: *mut c_void) {
        assert!(argument.is_null());
        ORDER
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n * 10 + 3))
            .unwrap();
    }
    #[test]
    fn quick_callbacks_have_separate_lifo_order_and_dso_lifetime() {
        let saved = core::mem::take(&mut *exit_handlers().lock().unwrap());
        ORDER.store(0, Ordering::SeqCst);
        unsafe {
            assert_eq!(kinakaze_abi_atexit(Some(normal)), 0);
            assert_eq!(kinakaze_abi_at_quick_exit(Some(quick)), 0);
            assert_eq!(
                kinakaze_abi___cxa_at_quick_exit(Some(cxa), 0xabcusize as _),
                0
            );
        }
        run_handlers(true);
        assert_eq!(ORDER.load(Ordering::SeqCst), 32);
        assert_eq!(exit_handlers().lock().unwrap().len(), 1);
        run_handlers(false);
        assert_eq!(ORDER.load(Ordering::SeqCst), 321);
        unsafe {
            kinakaze_abi___cxa_at_quick_exit(Some(cxa), 0xabcusize as _);
            kinakaze_abi___cxa_finalize(0xabcusize as _);
        }
        run_handlers(true);
        assert_eq!(ORDER.load(Ordering::SeqCst), 321);
        *exit_handlers().lock().unwrap() = saved;
    }
}
