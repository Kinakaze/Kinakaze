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

/// Invokes one handler.
///
/// # Safety
///
/// The registrar promised the function stays callable until the process exits.
unsafe fn run(handler: Handler) {
    match handler {
        // SAFETY: forwarded from this function's contract.
        Handler::Plain(function) => unsafe { function() },
        Handler::Cxa {
            function, argument, ..
        } => {
            // SAFETY: forwarded from this function's contract. The argument is
            // the one supplied at registration.
            unsafe { function(argument) }
        }
    }
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
                    Handler::Plain(_) => false,
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
}

/// Runs `atexit` handlers and flushes streams, then terminates.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_exit(status: c_int) -> ! {
    // The initial thread's C++ thread_local objects precede process atexit
    // handlers. Pthread key destructors belong to thread exit and do not run
    // as part of this process-exit path.
    kinakaze_tls::run_cxx_thread_destructors();
    // C requires handlers to run in reverse registration order. The lock is
    // released before each call so a handler may itself call `atexit`.
    loop {
        let next = match exit_handlers().lock() {
            Ok(mut handlers) => handlers.pop(),
            Err(_) => None,
        };
        match next {
            // SAFETY: the registrar promised the handler stays callable.
            Some(handler) => unsafe { run(handler) },
            None => break,
        }
    }
    // Buffered output must reach the descriptors before the process dies.
    let _ = crate::stdio::flush_all();
    super::kinakaze_abi__exit(status)
}
