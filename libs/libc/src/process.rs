//! Process lifetime, conversions, environment and sorting.
//!
//! The pieces of `<stdlib.h>` that are not allocation. `exit` matters most: C
//! requires it to run `atexit` handlers and flush every stream, so buffered
//! output would otherwise be lost on a normal return from `main`.

mod extended;
mod floating;
mod integer;

/// Linux tracing is not implemented by the process manager. Keep optional
/// libdw backtrace support loadable and let callers detect the unavailable
/// tracing operation through errno, without claiming an attached tracer.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ptrace(
    _request: i32,
    _pid: i32,
    _address: usize,
    _data: usize,
) -> i64 {
    crate::set_errno(kinakaze_vfs::ENOSYS);
    -1
}

use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;
#[cfg(test)]
use std::sync::Mutex;

mod termination;
pub use termination::{
    CxaHandler, ExitHandler, kinakaze_abi___cxa_atexit, kinakaze_abi___cxa_finalize,
    kinakaze_abi_atexit, kinakaze_abi_exit,
};
#[cfg(test)]
use termination::{Handler, exit_handlers};

/// Terminates immediately, without handlers or flushing.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi__exit(status: c_int) -> ! {
    if std::env::var_os("KINAKAZE_REPORT_TRAPS").is_some() {
        eprintln!(
            "kinakaze: libc _exit({status}) in host pid {}",
            std::process::id()
        );
    }
    // Publish the exact Linux-visible status before the Windows process object
    // can disappear. This is also what lets the real parent collect a child
    // created by `CLONE_PARENT` without borrowing the creator's local handle.
    kinakaze_vfs::job::publish_exit(status);
    // A real vfork parent remains suspended until this point.
    crate::exec::complete_vfork(false);
    terminate_host_process(status as u32)
}

/// A signal death is not `_exit(128 + signal)`: the parent must observe
/// WIFSIGNALED, not WIFEXITED. Publish exactly one terminal reason, then perform
/// the same vfork rendezvous and immediate host termination as ordinary exit.
pub(crate) fn terminate_from_signal(signal: i32) {
    kinakaze_vfs::job::publish_termination(signal);
    crate::exec::complete_vfork(false);
    terminate_host_process((128 + signal) as u32)
}

/// Kernel-style process termination for `_exit` and successful `exec` wrappers.
///
/// `ExitProcess` runs PE DLL detach and Rust TLS destructors. Linux `_exit`
/// explicitly runs no user-space teardown, and a fork child has inherited Rust
/// TLS bookkeeping that cannot safely be destructed as if Windows had created
/// the thread normally. `TerminateProcess` is the Windows primitive with the
/// matching immediate semantics.
pub(crate) fn terminate_host_process(status: u32) -> ! {
    if let Some(directory) = std::env::var_os("KINAKAZE_NAMESPACE_TRACE_DIR") {
        use std::io::Write;
        let path = std::path::PathBuf::from(directory)
            .join(format!("process-exit-{}.log", std::process::id()));
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "host_status={status}");
        }
    }
    use windows_sys::Win32::System::Threading::{ExitProcess, GetCurrentProcess, TerminateProcess};

    unsafe {
        let _ = TerminateProcess(GetCurrentProcess(), status);
        // Only reached if the immediate termination request failed.
        ExitProcess(status)
    }
}

/// `abort`, which terminates without flushing.
///
/// POSIX requires an abnormal termination status; 134 is what a shell reports
/// for a process killed by `SIGABRT`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_abort() -> ! {
    kinakaze_abi__exit(134)
}

/// Parses a leading signed integer, ignoring overflow as C's `atoi` does.
///
/// # Safety
///
/// `text` must be null-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_atoi(text: *const c_char) -> c_int {
    // SAFETY: forwarded from this function's contract.
    unsafe { kinakaze_abi_strtol(text, ptr::null_mut(), 10) as c_int }
}

/// `atol`.
///
/// # Safety
///
/// `text` must be null-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_atol(text: *const c_char) -> i64 {
    // SAFETY: forwarded from this function's contract.
    unsafe { kinakaze_abi_strtol(text, ptr::null_mut(), 10) }
}

/// `strtol`.
///
/// Sets `end` to the first unconverted character, or to `text` when no digits
/// were found, which is how callers detect failure.
///
/// # Safety
///
/// `text` must be null-terminated and `end` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtol(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> i64 {
    unsafe { integer::parse(text, end, base, true) as i64 }
}

/// `strtoul`.
///
/// # Safety
///
/// `text` must be null-terminated and `end` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtoul(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> u64 {
    // SAFETY: forwarded from this function's contract.
    unsafe { integer::parse(text, end, base, false) }
}

/// `atof`.
///
/// # Safety
///
/// `text` must be null-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_atof(text: *const c_char) -> f64 {
    // SAFETY: forwarded from this function's contract.
    unsafe { kinakaze_abi_strtod(text, ptr::null_mut()) }
}

/// `strtod` with C decimal/hex grammar, range errors and exact end pointers.
///
/// # Safety
/// `text` must be null-terminated and `end` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtod(
    text: *const c_char,
    end: *mut *const c_char,
) -> f64 {
    unsafe { floating::double(text, end) }
}

/// `abs`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_abs(value: c_int) -> c_int {
    value.wrapping_abs()
}

/// `labs`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_labs(value: i64) -> i64 {
    value.wrapping_abs()
}

/// `llabs`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_llabs(value: i64) -> i64 {
    value.wrapping_abs()
}

/// `strtoll` — parse a signed 64-bit integer.
///
/// # Safety
/// `text` must be null-terminated, `end` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtoll(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> i64 {
    // SAFETY: forwarded from this function's contract.
    unsafe { kinakaze_abi_strtol(text, end, base) }
}

/// `strtoull` — parse an unsigned 64-bit integer.
///
/// # Safety
/// `text` must be null-terminated, `end` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtoull(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> u64 {
    // SAFETY: forwarded from this function's contract.
    unsafe { integer::parse(text, end, base, false) }
}

/// `strtoimax` — parse a signed intmax_t (same as strtoll on LP64).
///
/// # Safety
/// `text` must be null-terminated, `end` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtoimax(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> i64 {
    unsafe { kinakaze_abi_strtol(text, end, base) }
}

/// `strtoumax` — parse an unsigned uintmax_t (same as strtoull on LP64).
///
/// # Safety
/// `text` must be null-terminated, `end` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtoumax(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> u64 {
    unsafe { integer::parse(text, end, base, false) }
}

/// `strtof` — parse a float.
///
/// # Safety
/// `text` must be null-terminated, `end` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtof(
    text: *const c_char,
    end: *mut *const c_char,
) -> f32 {
    // SAFETY: forwarded from this function's contract.
    unsafe { floating::float(text, end) }
}

/// `strtof128` — not a standard C function, returns f64 as best approximation.
///
/// # Safety
/// `text` must be null-terminated, `end` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtof128(
    text: *const c_char,
    end: *mut *const c_char,
) -> f64 {
    unsafe { kinakaze_abi_strtod(text, end) }
}

/// `strfromf128` — format f128 (not standard; formats f64 value).
///
/// # Safety
/// `buf` must be `size` bytes writable, `fmt` must be null-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strfromf128(
    buf: *mut c_char,
    size: usize,
    _fmt: *const c_char,
    value: f64,
) -> c_int {
    if buf.is_null() || size == 0 {
        return 0;
    }
    use std::io::Write;
    let mut out = Vec::new();
    let _ = write!(out, "{}", value);
    let len = out.len().min(size.saturating_sub(1));
    unsafe {
        ptr::copy_nonoverlapping(out.as_ptr().cast::<c_char>(), buf, len);
        *buf.add(len) = 0;
    }
    out.len() as c_int
}

/// C89 `div`: divide and get quotient+remainder in one struct.
#[repr(C)]
pub struct DivT {
    pub quot: c_int,
    pub rem: c_int,
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_div(numer: c_int, denom: c_int) -> DivT {
    DivT {
        quot: numer / denom,
        rem: numer % denom,
    }
}

/// `ldiv`.
#[repr(C)]
pub struct LDivT {
    pub quot: i64,
    pub rem: i64,
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ldiv(numer: i64, denom: i64) -> LDivT {
    LDivT {
        quot: numer / denom,
        rem: numer % denom,
    }
}

/// `lldiv`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_lldiv(numer: i64, denom: i64) -> LDivT {
    LDivT {
        quot: numer / denom,
        rem: numer % denom,
    }
}

/// `isalpha` — is the character an ASCII letter?
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_isalpha(c: c_int) -> c_int {
    ((c as u8).is_ascii_alphabetic()) as c_int
}

/// `isalnum` — is the character alphanumeric?
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_isalnum(c: c_int) -> c_int {
    ((c as u8).is_ascii_alphanumeric()) as c_int
}

/// `isdigit`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_isdigit(c: c_int) -> c_int {
    ((c as u8).is_ascii_digit()) as c_int
}

/// `isxdigit`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_isxdigit(c: c_int) -> c_int {
    ((c as u8).is_ascii_hexdigit()) as c_int
}

/// `isspace`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_isspace(c: c_int) -> c_int {
    ((c as u8).is_ascii_whitespace()) as c_int
}

/// `isprint`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_isprint(c: c_int) -> c_int {
    let b = c as u8;
    (b >= 0x20 && b < 0x7f) as c_int
}

/// `isgraph`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_isgraph(c: c_int) -> c_int {
    let b = c as u8;
    (b > 0x20 && b < 0x7f) as c_int
}

/// `ispunct`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ispunct(c: c_int) -> c_int {
    let b = c as u8;
    (b.is_ascii_punctuation()) as c_int
}

/// `iscntrl`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_iscntrl(c: c_int) -> c_int {
    let b = c as u8;
    (b.is_ascii_control()) as c_int
}

/// `isupper`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_isupper(c: c_int) -> c_int {
    ((c as u8).is_ascii_uppercase()) as c_int
}

/// `islower`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_islower(c: c_int) -> c_int {
    ((c as u8).is_ascii_lowercase()) as c_int
}

/// `isblank`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_isblank(c: c_int) -> c_int {
    (c == b' ' as c_int || c == b'\t' as c_int) as c_int
}

/// `isascii`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_isascii(c: c_int) -> c_int {
    ((c as u32) < 128) as c_int
}

/// `toupper`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_toupper(c: c_int) -> c_int {
    let b = c as u8;
    if b.is_ascii_lowercase() {
        (b - 32) as c_int
    } else {
        c
    }
}

/// `tolower`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_tolower(c: c_int) -> c_int {
    let b = c as u8;
    if b.is_ascii_uppercase() {
        (b + 32) as c_int
    } else {
        c
    }
}

/// `toascii`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_toascii(c: c_int) -> c_int {
    c & 0x7f
}

mod environment;
pub(crate) use environment::adopt;
pub use environment::*;

/// A `qsort` comparison callback.
pub type Comparator = unsafe extern "sysv64" fn(*const c_void, *const c_void) -> c_int;

/// `qsort`.
///
/// Sorting is delegated to the standard library's sort, which is why the
/// elements are first collected into a temporary buffer of boxed byte runs: the
/// comparator works on addresses, so the elements must stay put while a
/// comparison is in flight.
///
/// # Safety
///
/// `base` must hold `count` elements of `size` bytes and `comparator` must be a
/// valid callback that does not modify the array.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_qsort(
    base: *mut c_void,
    count: usize,
    size: usize,
    comparator: Option<Comparator>,
) {
    let Some(comparator) = comparator else {
        return;
    };
    if base.is_null() || size == 0 || count < 2 {
        return;
    }

    // Copy the elements out, sort the copies by comparing them in place, then
    // write them back. Comparing copies is required because the standard sort
    // moves elements around during the sort.
    let total = match count.checked_mul(size) {
        Some(total) => total,
        None => return,
    };
    // SAFETY: the caller guarantees the array covers `total` bytes.
    let bytes = unsafe { core::slice::from_raw_parts(base.cast::<u8>(), total) };
    let mut elements: Vec<Vec<u8>> = bytes.chunks(size).map(<[u8]>::to_vec).collect();

    elements.sort_by(|left, right| {
        // SAFETY: both vectors hold `size` bytes and outlive the call.
        let order = unsafe { comparator(left.as_ptr().cast(), right.as_ptr().cast()) };
        order.cmp(&0)
    });

    for (index, element) in elements.iter().enumerate() {
        // SAFETY: `index * size` stays within the original array.
        unsafe {
            ptr::copy_nonoverlapping(element.as_ptr(), base.cast::<u8>().add(index * size), size)
        };
    }
}

/// `bsearch`.
///
/// # Safety
///
/// `base` must hold `count` sorted elements of `size` bytes and `comparator`
/// must be a valid callback.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_bsearch(
    key: *const c_void,
    base: *const c_void,
    count: usize,
    size: usize,
    comparator: Option<Comparator>,
) -> *mut c_void {
    let Some(comparator) = comparator else {
        return ptr::null_mut();
    };
    if key.is_null() || base.is_null() || size == 0 {
        return ptr::null_mut();
    }
    let mut low = 0usize;
    let mut high = count;
    while low < high {
        let middle = low + (high - low) / 2;
        // SAFETY: `middle` is below `count`, so the element is in range.
        let element = unsafe { base.cast::<u8>().add(middle * size) };
        // SAFETY: both pointers address `size`-byte elements.
        let order = unsafe { comparator(key, element.cast()) };
        match order.cmp(&0) {
            core::cmp::Ordering::Equal => return element as *mut c_void,
            core::cmp::Ordering::Less => high = middle,
            core::cmp::Ordering::Greater => low = middle + 1,
        }
    }
    ptr::null_mut()
}

/// `getpid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getpid() -> c_int {
    kinakaze_runtime::job::namespaces::visible(kinakaze_vfs::job::process_id()).unwrap_or(0)
        as c_int
}

/// `getppid`, read from the shared Linux PID namespace.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getppid() -> c_int {
    kinakaze_runtime::job::namespaces::visible(kinakaze_vfs::job::parent_process_id()).unwrap_or(0)
        as c_int
}

/// `sleep`, returning the number of seconds left unslept.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sleep(seconds: u32) -> u32 {
    // Not yet interruptible: without signal delivery there is nothing that can
    // cut the wait short, so the full duration always elapses.
    std::thread::sleep(std::time::Duration::from_secs(seconds as u64));
    0
}

/// `usleep`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_usleep(microseconds: u32) -> c_int {
    std::thread::sleep(std::time::Duration::from_micros(microseconds as u64));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the handler tests.
    ///
    /// The registered-handler list is process-global, and `__cxa_finalize(null)`
    /// is defined to run every C++ handler no matter which object owns it. Tests
    /// that share that list therefore cannot run concurrently.
    static HANDLER_TESTS: Mutex<()> = Mutex::new(());

    /// Where the test handlers record the order they ran in.
    static ORDER: Mutex<Vec<c_int>> = Mutex::new(Vec::new());

    #[test]
    fn exit_runs_cxx_before_atexit_without_pthread_keys() {
        const OUTPUT: &str = "KINAKAZE_TEST_PROCESS_EXIT_ORDER";
        fn append(value: &[u8]) {
            use std::io::Write;
            let path = std::env::var_os(OUTPUT).expect("subprocess output path");
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("open teardown log")
                .write_all(value)
                .expect("write teardown log");
        }
        unsafe extern "sysv64" fn cxx(_: *mut c_void) {
            append(b"tls\n");
        }
        unsafe extern "sysv64" fn key(_: *mut c_void) {
            append(b"key\n");
        }
        unsafe extern "sysv64" fn at_exit() {
            append(b"atexit\n");
        }

        if std::env::var_os(OUTPUT).is_some() {
            kinakaze_tls::cxa_thread_atexit(Some(cxx), ptr::null_mut(), ptr::null_mut()).unwrap();
            let index = kinakaze_tls::pthread_key_create(Some(key)).unwrap();
            assert_eq!(
                kinakaze_tls::pthread_setspecific(index, 1usize as *mut c_void),
                Ok(())
            );
            assert_eq!(unsafe { kinakaze_abi_atexit(Some(at_exit)) }, 0);
            kinakaze_abi_exit(42);
        }

        use std::os::windows::process::CommandExt;
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "kinakaze-v2-exit-{}-{unique}.log",
            std::process::id()
        ));
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "process::tests::exit_runs_cxx_before_atexit_without_pthread_keys",
                "--nocapture",
            ])
            .env(OUTPUT, &path)
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        let written = std::fs::read_to_string(&path);
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        assert_eq!(
            result.status.code(),
            Some(42),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(written.unwrap(), "tls\natexit\n");
    }

    /// A `__cxa_atexit` handler that records the identifier it was given.
    ///
    /// This is also what checks that the registered argument is delivered
    /// unchanged: the identifier the handler reports is the one passed to
    /// `__cxa_atexit`.
    ///
    /// # Safety
    ///
    /// Matches the `CxaHandler` signature; the argument is a token, not a pointer
    /// that gets dereferenced.
    unsafe extern "sysv64" fn record(argument: *mut c_void) {
        if let Ok(mut order) = ORDER.lock() {
            order.push(argument as usize as c_int);
        }
    }

    /// A plain `atexit` handler, which must survive `__cxa_finalize`.
    ///
    /// # Safety
    ///
    /// Matches the `ExitHandler` signature.
    unsafe extern "sysv64" fn plain() {
        if let Ok(mut order) = ORDER.lock() {
            order.push(-1);
        }
    }

    /// Registers one handler per identifier against `dso`.
    fn register_all(dso: *mut c_void, identifiers: &[c_int]) {
        for identifier in identifiers {
            // SAFETY: `record` is a live function and the argument is a token.
            let result = unsafe {
                kinakaze_abi___cxa_atexit(Some(record), *identifier as usize as *mut c_void, dso)
            };
            assert_eq!(result, 0);
        }
    }

    /// Clears the recording buffer.
    fn reset() {
        ORDER.lock().expect("order lock").clear();
    }

    /// Runs the handlers of `dso` and returns the order they ran in.
    fn finalize(dso: *mut c_void) -> Vec<c_int> {
        // SAFETY: every registered handler is still callable.
        unsafe { kinakaze_abi___cxa_finalize(dso) };
        ORDER.lock().expect("order lock").clone()
    }

    /// Counts the plain `atexit` registrations still pending.
    fn plain_registrations() -> usize {
        exit_handlers()
            .lock()
            .expect("handler lock")
            .iter()
            .filter(|handler| matches!(handler, Handler::Plain(_)))
            .count()
    }

    #[test]
    fn cxa_handlers_run_in_reverse_registration_order() {
        let _guard = HANDLER_TESTS.lock().expect("test lock");
        reset();
        let dso = 0x1000usize as *mut c_void;
        register_all(dso, &[1, 2, 3]);
        // Last registered runs first, which is what C requires.
        assert_eq!(finalize(dso), vec![3, 2, 1]);
    }

    #[test]
    fn cxa_finalize_selects_by_dso_handle() {
        let _guard = HANDLER_TESTS.lock().expect("test lock");
        reset();
        let mine = 0x2000usize as *mut c_void;
        let other = 0x3000usize as *mut c_void;
        register_all(mine, &[10, 11]);
        register_all(other, &[20]);

        // Only this object's handlers run, and they are removed by doing so.
        assert_eq!(finalize(mine), vec![11, 10]);
        assert_eq!(
            finalize(mine),
            vec![11, 10],
            "a second finalize re-ran them"
        );
        // The other object's handler was left untouched, and runs on its own
        // finalize.
        assert_eq!(finalize(other), vec![11, 10, 20]);
    }

    #[test]
    fn cxa_finalize_with_a_null_handle_runs_every_object() {
        let _guard = HANDLER_TESTS.lock().expect("test lock");
        reset();
        register_all(0x5000usize as *mut c_void, &[40]);
        register_all(0x6000usize as *mut c_void, &[41]);
        // A null handle ignores ownership and still works in reverse order.
        assert_eq!(finalize(ptr::null_mut()), vec![41, 40]);
    }

    #[test]
    fn cxa_finalize_leaves_plain_atexit_handlers_alone() {
        let _guard = HANDLER_TESTS.lock().expect("test lock");
        reset();
        // Handlers registered through `atexit` belong to the program, not to any
        // one object, so finalizing an object must not consume them.
        let before = plain_registrations();
        // SAFETY: `plain` is a live function.
        assert_eq!(unsafe { kinakaze_abi_atexit(Some(plain)) }, 0);
        assert_eq!(plain_registrations(), before + 1);

        let dso = 0x4000usize as *mut c_void;
        register_all(dso, &[30]);
        // Only the C++ handler ran.
        assert_eq!(finalize(dso), vec![30]);
        // And the plain one is still pending.
        assert_eq!(plain_registrations(), before + 1);
    }

    #[test]
    fn getenv_reads_the_published_environ_block() {
        // A distinct name per test keeps the shared process environment from
        // making the tests interfere with each other.
        let name = c"KINAKAZE_TEST_GETENV";
        let value = c"published";
        // SAFETY: both arguments are null-terminated literals.
        let result = unsafe { kinakaze_abi_setenv(name.as_ptr(), value.as_ptr(), 1) };
        assert_eq!(result, 0);

        // SAFETY: the name is a null-terminated literal.
        let found = unsafe { kinakaze_abi_getenv(name.as_ptr()) };
        assert!(!found.is_null());
        // SAFETY: `getenv` returns a pointer into a null-terminated entry.
        assert_eq!(unsafe { CStr::from_ptr(found) }, value);

        // The block `environ` names must contain the same binding, so that a
        // guest walking it by hand agrees with `getenv`.
        let block = kinakaze_abi_environ.get();
        assert!(!block.is_null());
        let mut cursor = 0;
        let mut seen = false;
        loop {
            // SAFETY: `environ` is a null-terminated array of C strings.
            let entry = unsafe { *block.add(cursor) };
            if entry.is_null() {
                break;
            }
            // SAFETY: entries are null-terminated strings.
            if unsafe { CStr::from_ptr(entry) } == c"KINAKAZE_TEST_GETENV=published" {
                seen = true;
            }
            cursor += 1;
        }
        assert!(seen, "environ does not contain the binding getenv reported");

        // SAFETY: the name is a null-terminated literal.
        assert_eq!(unsafe { kinakaze_abi_unsetenv(name.as_ptr()) }, 0);
        // SAFETY: the name is a null-terminated literal.
        assert!(unsafe { kinakaze_abi_getenv(name.as_ptr()) }.is_null());
    }

    #[test]
    fn setenv_honours_the_overwrite_flag() {
        let name = c"KINAKAZE_TEST_OVERWRITE";
        // SAFETY: null-terminated literals.
        unsafe {
            assert_eq!(kinakaze_abi_setenv(name.as_ptr(), c"first".as_ptr(), 1), 0);
            // A zero flag must leave the existing binding in place.
            assert_eq!(kinakaze_abi_setenv(name.as_ptr(), c"second".as_ptr(), 0), 0);
        }
        // SAFETY: null-terminated literal.
        let found = unsafe { kinakaze_abi_getenv(name.as_ptr()) };
        // SAFETY: `getenv` returned a pointer into a null-terminated entry.
        assert_eq!(unsafe { CStr::from_ptr(found) }, c"first");

        // SAFETY: null-terminated literals.
        unsafe {
            assert_eq!(kinakaze_abi_setenv(name.as_ptr(), c"second".as_ptr(), 1), 0);
        }
        // SAFETY: null-terminated literal.
        let found = unsafe { kinakaze_abi_getenv(name.as_ptr()) };
        // SAFETY: `getenv` returned a pointer into a null-terminated entry.
        assert_eq!(unsafe { CStr::from_ptr(found) }, c"second");
        // SAFETY: null-terminated literal.
        unsafe { kinakaze_abi_unsetenv(name.as_ptr()) };
    }
}
