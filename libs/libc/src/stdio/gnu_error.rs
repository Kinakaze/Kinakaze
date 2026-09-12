//! GNU diagnostics and COPY-visible controls; callbacks run without private locks.
use super::exports::{kinakaze_stderr, kinakaze_stdout};
use super::*;
use crate::copied::CopiedValue;
use core::ffi::CStr;

type Prefix = unsafe extern "sysv64" fn();
#[unsafe(no_mangle)]
pub static kinakaze_abi_error_message_count: CopiedValue<u32> = CopiedValue::new(0);
#[unsafe(no_mangle)]
pub static kinakaze_abi_error_print_progname: CopiedValue<Option<Prefix>> = CopiedValue::new(None);
#[unsafe(no_mangle)]
pub static kinakaze_abi_error_one_per_line: CopiedValue<i32> = CopiedValue::new(0);

// GNU retains the caller's filename pointer for error_one_per_line. As with
// glibc, the caller must keep it readable until a later diagnostic replaces it.
static PREVIOUS: Mutex<(usize, u32)> = Mutex::new((0, 0));

unsafe fn repeated(filename: *const c_char, line: u32) -> bool {
    if unsafe { kinakaze_abi_error_one_per_line.get() } == 0 {
        return false;
    }
    let mut old = PREVIOUS.lock().unwrap_or_else(|e| e.into_inner());
    let same = old.0 == filename as usize
        || (old.0 != 0
            && !filename.is_null()
            && unsafe { CStr::from_ptr(old.0 as *const c_char) == CStr::from_ptr(filename) });
    if old.1 == line && same {
        return true;
    }
    *old = (filename as usize, line);
    false
}

pub(crate) unsafe fn redirect(name: &str, target: *mut c_void) -> bool {
    unsafe {
        match name {
            "error_message_count" => kinakaze_abi_error_message_count.redirect(target.cast()),
            "error_print_progname" => kinakaze_abi_error_print_progname.redirect(target.cast()),
            "error_one_per_line" => kinakaze_abi_error_one_per_line.redirect(target.cast()),
            _ => return false,
        }
    }
    true
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ForkState {
    magic: [u8; 8],
    count: u32,
    one_per_line: i32,
    prefix: Option<Prefix>,
    targets: [usize; 3],
    filename: usize,
    line: u32,
    reserved: u32,
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    let size = size_of::<ForkState>();
    if output.is_null() {
        return size as isize;
    }
    if capacity < size {
        return -22;
    }
    let old = match PREVIOUS.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return -16,
    };
    unsafe {
        output.cast::<ForkState>().write_unaligned(ForkState {
            magic: *b"CRYERR01",
            count: kinakaze_abi_error_message_count.get(),
            one_per_line: kinakaze_abi_error_one_per_line.get(),
            prefix: kinakaze_abi_error_print_progname.get(),
            targets: [
                kinakaze_abi_error_message_count.target() as usize,
                kinakaze_abi_error_one_per_line.target() as usize,
                kinakaze_abi_error_print_progname.target() as usize,
            ],
            filename: old.0,
            line: old.1,
            reserved: 0,
        });
    }
    size as isize
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length != size_of::<ForkState>() {
        return 22;
    }
    let state = unsafe { input.cast::<ForkState>().read_unaligned() };
    if state.magic != *b"CRYERR01" || state.reserved != 0 {
        return 22;
    }
    unsafe {
        kinakaze_abi_error_message_count.redirect(state.targets[0] as *mut u32);
        kinakaze_abi_error_one_per_line.redirect(state.targets[1] as *mut i32);
        kinakaze_abi_error_print_progname.redirect(state.targets[2] as *mut Option<Prefix>);
        kinakaze_abi_error_message_count.set(state.count);
        kinakaze_abi_error_one_per_line.set(state.one_per_line);
        kinakaze_abi_error_print_progname.set(state.prefix);
    }
    *PREVIOUS.lock().unwrap_or_else(|e| e.into_inner()) = (state.filename, state.line);
    0
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 30,
        key: 0x4352_5945_5252_3031,
        prepare: None,
        snapshot: Some(snapshot),
        parent: None,
        child: Some(child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static REGISTER: extern "C" fn() = register;
/// Formatting/termination behind the GNU `error` variadic entry point.
///
/// # Safety
///
/// The assembly thunk supplies the live System V va_list for `fmt`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_error_impl(
    status: c_int,
    errnum: c_int,
    fmt: *const c_char,
    arguments: *mut VaList,
) {
    unsafe {
        report(
            status,
            errnum,
            core::ptr::null(),
            0,
            fmt,
            arguments,
            false,
            true,
        );
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_error_at_line_impl(
    status: c_int,
    errnum: c_int,
    filename: *const c_char,
    line: u32,
    fmt: *const c_char,
    arguments: *mut VaList,
) {
    unsafe { report(status, errnum, filename, line, fmt, arguments, true, true) };
}

unsafe fn report(
    status: c_int,
    errnum: c_int,
    filename: *const c_char,
    line: u32,
    fmt: *const c_char,
    arguments: *mut VaList,
    at_line: bool,
    counted: bool,
) {
    if at_line && unsafe { repeated(filename, line) } {
        return;
    }
    // GNU error flushes pending stdout before reporting on stderr.
    let _ = unsafe { super::fflush(kinakaze_stdout()) };
    let err_stream = kinakaze_stderr();
    let prefix = if counted {
        unsafe { kinakaze_abi_error_print_progname.get() }
    } else {
        None
    };
    if let Some(prefix) = prefix {
        unsafe { prefix() };
    } else {
        let program = crate::misc::program_invocation_name.get();
        if !program.is_null() {
            let _ = unsafe { super::fputs(program, err_stream) };
            let suffix = if at_line { c":" } else { c": " };
            let _ = unsafe { super::fputs(suffix.as_ptr(), err_stream) };
        }
    }
    if at_line && filename.is_null() {
        let _ = unsafe { super::fputs(c" ".as_ptr(), err_stream) };
    }
    if !filename.is_null() {
        let _ = unsafe { super::fputs(filename, err_stream) };
        let _ = unsafe { super::fputs(c":".as_ptr(), err_stream) };
        let mut bytes = [0u8; 10];
        let mut number = line;
        let mut at = bytes.len();
        loop {
            at -= 1;
            bytes[at] = b'0' + (number % 10) as u8;
            number /= 10;
            if number == 0 {
                break;
            }
        }
        let _ = unsafe {
            super::fwrite(
                bytes.as_ptr().add(at).cast(),
                1,
                bytes.len() - at,
                err_stream,
            )
        };
        let _ = unsafe { super::fputs(c": ".as_ptr(), err_stream) };
    }
    if !fmt.is_null() && !arguments.is_null() {
        let _ = unsafe { stream_format(err_stream, fmt, &mut *arguments) };
    }
    if counted {
        let _guard = PREVIOUS.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            kinakaze_abi_error_message_count
                .set(kinakaze_abi_error_message_count.get().wrapping_add(1))
        };
    }
    if errnum != 0 {
        let s = unsafe { crate::string::strerror(errnum) };
        let colon = b": \0".as_ptr().cast();
        let _ = unsafe { super::fputs(colon, err_stream) };
        let _ = unsafe { super::fputs(s, err_stream) };
    }
    let newline = b"\n\0".as_ptr().cast();
    let _ = unsafe { super::fputs(newline, err_stream) };
    let _ = unsafe { super::fflush(err_stream) };
    if status != 0 {
        crate::process::kinakaze_abi_exit(status);
    }
}

pub(super) unsafe fn warn(errnum: c_int, fmt: *const c_char, arguments: *mut VaList) {
    unsafe {
        report(
            0,
            errnum,
            core::ptr::null(),
            0,
            fmt,
            arguments,
            false,
            false,
        )
    };
}
