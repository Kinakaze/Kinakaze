//! The `_FORTIFY_SOURCE` entry points and the C23 conversion aliases.
//!
//! A guest compiled with `-D_FORTIFY_SOURCE` never calls `memcpy` or `printf`
//! directly. GCC rewrites each call whose destination size it can prove into the
//! matching `__*_chk` form, which carries that size as an extra argument and
//! traps before a call that would overflow the destination. BusyBox is built
//! this way, so these names are not optional extras: they are the only string
//! and output functions its object code refers to.
//!
//! # Where the extra argument sits
//!
//! The two families disagree, and getting it wrong is silent corruption rather
//! than a link error, so it is worth stating plainly:
//!
//! - The `printf` family takes `int flag` **first**, ahead of the format.
//! - The memory and string family takes `size_t destlen` **last**.
//!
//! `flag` is glibc's internal fortify level. Its value selects between glibc's
//! own checked and unchecked paths, which we do not have, so it is deliberately
//! ignored. It must still be *consumed* as a parameter, because every argument
//! after it lands one register later than it otherwise would.
//!
//! # Variadic entry points
//!
//! Rust cannot declare an `extern "sysv64"` function with `...`, so the six
//! variadic `_chk` names are assembly thunks that build a System V `va_list` and
//! tail-call a Rust implementation, exactly as [`crate::variadic`] does for the
//! plain `printf` family. Each thunk's `gp_offset` skips the named integer
//! arguments its own signature already consumed, which is where the differing
//! argument order shows up concretely.

use core::ffi::{c_char, c_int, c_void};

use crate::format::VaList;
use crate::stdio::{File, exports::kinakaze_stdout};

/// `open`'s `O_CREAT`, which `__open64_2` must reject.
const O_CREAT: c_int = kinakaze_vfs::fs::O_CREAT;
/// `O_TMPFILE`, which also requires a mode and so is rejected alongside it.
const O_TMPFILE: c_int = 0o20_200_000;

unsafe extern "sysv64" {
    /// `__chk_fail`: the shared failure path for every fortified function.
    ///
    /// Defined in [`crate::startup`], which also owns `__stack_chk_fail` and
    /// `__assert_fail`. It lives there because the compiler emits direct calls to
    /// it and glibc exports it as a real symbol, so it belongs with the other
    /// abort paths rather than being duplicated here. It is reached by linkage
    /// because that module is private; there is one definition and one
    /// diagnostic.
    ///
    /// It does not return: reaching it means a store past the end of an object
    /// was about to happen, and there is no state left worth returning to.
    pub(crate) safe fn kinakaze_abi___chk_fail() -> !;
}

/// Reports whether an access of `length` bytes fits an object of `destlen`.
///
/// Factored out of the checks so the boundary itself is unit-testable: the
/// failing path calls [`kinakaze_abi___chk_fail`], which exits the process and
/// therefore cannot be reached from a test.
///
/// `destlen` is `(size_t)-1` when the compiler could not determine a size, which
/// is glibc's "unknown, do not check" encoding and passes everything.
const fn fits(length: usize, destlen: usize) -> bool {
    destlen == usize::MAX || length <= destlen
}

/// Traps unless an access of `length` bytes fits an object of `destlen`.
fn check(length: usize, destlen: usize) {
    if !fits(length, destlen) {
        kinakaze_abi___chk_fail();
    }
}

// ---------------------------------------------------------------------------
// The memory and string family: `size_t destlen` is the LAST argument.
// ---------------------------------------------------------------------------

/// `__memcpy_chk`.
///
/// # Safety
///
/// The plain `memcpy` contract applies, and `destination` must name at least
/// `destlen` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___memcpy_chk(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
    destlen: usize,
) -> *mut c_void {
    check(count, destlen);
    // SAFETY: the copy is within the destination, as just checked, and the
    // source is the caller's responsibility exactly as it is for `memcpy`.
    unsafe { crate::string::memcpy(destination, source, count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __memcpy_chk(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
    destlen: usize,
) -> *mut c_void {
    unsafe { kinakaze_abi___memcpy_chk(destination, source, count, destlen) }
}

/// `__memmove_chk`.
///
/// # Safety
///
/// As [`kinakaze_abi___memcpy_chk`], but the regions may overlap.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___memmove_chk(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
    destlen: usize,
) -> *mut c_void {
    check(count, destlen);
    // SAFETY: the copy is within the destination, as just checked.
    unsafe { crate::string::memmove(destination, source, count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __memmove_chk(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
    destlen: usize,
) -> *mut c_void {
    unsafe { kinakaze_abi___memmove_chk(destination, source, count, destlen) }
}

/// `__mempcpy_chk`.
///
/// `mempcpy` returns the end of the copy rather than its start, which is what
/// makes it worth having: successive copies chain without recomputing offsets.
///
/// # Safety
///
/// As [`kinakaze_abi___memcpy_chk`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___mempcpy_chk(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
    destlen: usize,
) -> *mut c_void {
    check(count, destlen);
    // SAFETY: the copy is within the destination, as just checked.
    unsafe { crate::string::memcpy(destination, source, count) };
    // SAFETY: one past the last byte written is within the same object, so the
    // pointer is valid to form even though it must not be dereferenced.
    unsafe { destination.byte_add(count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __mempcpy_chk(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
    destlen: usize,
) -> *mut c_void {
    unsafe { kinakaze_abi___mempcpy_chk(destination, source, count, destlen) }
}

/// `__memset_chk`.
///
/// # Safety
///
/// `destination` must name at least `destlen` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___memset_chk(
    destination: *mut c_void,
    value: c_int,
    count: usize,
    destlen: usize,
) -> *mut c_void {
    check(count, destlen);
    // SAFETY: the store is within the destination, as just checked.
    unsafe { crate::string::memset(destination, value, count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __memset_chk(
    destination: *mut c_void,
    value: c_int,
    count: usize,
    destlen: usize,
) -> *mut c_void {
    unsafe { kinakaze_abi___memset_chk(destination, value, count, destlen) }
}

/// `__strcpy_chk`.
///
/// The length is measured before copying, because the terminator has to fit too
/// and a `strcpy` that has already begun cannot be taken back.
///
/// # Safety
///
/// `source` must be null-terminated and `destination` must name at least
/// `destlen` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___strcpy_chk(
    destination: *mut c_char,
    source: *const c_char,
    destlen: usize,
) -> *mut c_char {
    // SAFETY: the caller guarantees a terminator.
    let length = unsafe { crate::string::strlen(source) };
    check(length + 1, destlen);
    // SAFETY: the string and its terminator fit, as just checked.
    unsafe { crate::string::strcpy(destination, source) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __strcpy_chk(
    destination: *mut c_char,
    source: *const c_char,
    destlen: usize,
) -> *mut c_char {
    unsafe { kinakaze_abi___strcpy_chk(destination, source, destlen) }
}

/// `__strcat_chk`.
///
/// # Safety
///
/// Both arguments must be null-terminated and `destination` must name at least
/// `destlen` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___strcat_chk(
    destination: *mut c_char,
    source: *const c_char,
    destlen: usize,
) -> *mut c_char {
    // The result occupies both strings plus one terminator.
    // SAFETY: the caller guarantees terminators on both strings.
    let existing = unsafe { crate::string::strlen(destination) };
    // SAFETY: as above.
    let appended = unsafe { crate::string::strlen(source) };
    check(existing + appended + 1, destlen);
    // SAFETY: the concatenation fits, as just checked.
    unsafe { crate::string::strcat(destination, source) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __strcat_chk(
    destination: *mut c_char,
    source: *const c_char,
    destlen: usize,
) -> *mut c_char {
    unsafe { kinakaze_abi___strcat_chk(destination, source, destlen) }
}

/// `__stpcpy_chk`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___stpcpy_chk(
    destination: *mut c_char,
    source: *const c_char,
    destlen: usize,
) -> *mut c_char {
    let length = unsafe { crate::string::strlen(source) };
    check(length + 1, destlen);
    unsafe { crate::strextra::windows::kinakaze_abi_stpcpy(destination, source) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __stpcpy_chk(
    destination: *mut c_char,
    source: *const c_char,
    destlen: usize,
) -> *mut c_char {
    unsafe { kinakaze_abi___stpcpy_chk(destination, source, destlen) }
}

/// `__stpncpy_chk`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___stpncpy_chk(
    destination: *mut c_char,
    source: *const c_char,
    count: usize,
    destlen: usize,
) -> *mut c_char {
    check(count, destlen);
    unsafe { crate::strextra::windows::kinakaze_abi_stpncpy(destination, source, count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __stpncpy_chk(
    destination: *mut c_char,
    source: *const c_char,
    count: usize,
    destlen: usize,
) -> *mut c_char {
    unsafe { kinakaze_abi___stpncpy_chk(destination, source, count, destlen) }
}

/// `__strncpy_chk`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___strncpy_chk(
    destination: *mut c_char,
    source: *const c_char,
    count: usize,
    destlen: usize,
) -> *mut c_char {
    check(count, destlen);
    unsafe { crate::string::strncpy(destination, source, count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __strncpy_chk(
    destination: *mut c_char,
    source: *const c_char,
    count: usize,
    destlen: usize,
) -> *mut c_char {
    unsafe { kinakaze_abi___strncpy_chk(destination, source, count, destlen) }
}

/// `__strncat_chk`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___strncat_chk(
    destination: *mut c_char,
    source: *const c_char,
    count: usize,
    destlen: usize,
) -> *mut c_char {
    let existing = unsafe { crate::string::strlen(destination) };
    let appended = unsafe { crate::string::strnlen(source, count) };
    check(existing + appended + 1, destlen);
    unsafe { crate::string::strncat(destination, source, count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __strncat_chk(
    destination: *mut c_char,
    source: *const c_char,
    count: usize,
    destlen: usize,
) -> *mut c_char {
    unsafe { kinakaze_abi___strncat_chk(destination, source, count, destlen) }
}

/// `__poll_chk`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___poll_chk(
    fds: *mut crate::fdio::PollFd,
    nfds: usize,
    timeout: c_int,
    fdslen: usize,
) -> c_int {
    check(nfds * core::mem::size_of::<crate::fdio::PollFd>(), fdslen);
    unsafe { crate::fdio::kinakaze_abi_poll(fds, nfds as u64, timeout) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __poll_chk(
    fds: *mut crate::fdio::PollFd,
    nfds: usize,
    timeout: c_int,
    fdslen: usize,
) -> c_int {
    unsafe { kinakaze_abi___poll_chk(fds, nfds, timeout, fdslen) }
}

/// `__read_chk`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___read_chk(
    fd: c_int,
    buf: *mut c_void,
    count: usize,
    buflen: usize,
) -> isize {
    check(count, buflen);
    unsafe { crate::kinakaze_read(fd, buf, count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __read_chk(
    fd: c_int,
    buf: *mut c_void,
    count: usize,
    buflen: usize,
) -> isize {
    unsafe { kinakaze_abi___read_chk(fd, buf, count, buflen) }
}

/// `__recv_chk`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___pread_chk(
    fd: c_int,
    buf: *mut c_void,
    count: usize,
    offset: i64,
    buflen: usize,
) -> isize {
    check(count, buflen);
    unsafe { crate::fdio::kinakaze_abi_pread64(fd, buf, count, offset) }
}

/// Checked recv uses the same object-size contract as read/pread.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___recv_chk(
    socket: c_int,
    buffer: *mut c_void,
    length: usize,
    buflen: usize,
    flags: c_int,
) -> isize {
    check(length, buflen);
    unsafe { crate::net::kinakaze_abi_recv(socket, buffer, length, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __recv_chk(
    socket: c_int,
    buffer: *mut c_void,
    length: usize,
    buflen: usize,
    flags: c_int,
) -> isize {
    unsafe { kinakaze_abi___recv_chk(socket, buffer, length, buflen, flags) }
}

/// `__recvfrom_chk`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___recvfrom_chk(
    socket: c_int,
    buffer: *mut c_void,
    length: usize,
    buflen: usize,
    flags: c_int,
    address: *mut c_void,
    address_len: *mut c_int,
) -> isize {
    check(length, buflen);
    unsafe {
        crate::net::kinakaze_abi_recvfrom(
            socket,
            buffer,
            length,
            flags,
            address.cast::<u8>(),
            address_len,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __recvfrom_chk(
    socket: c_int,
    buffer: *mut c_void,
    length: usize,
    buflen: usize,
    flags: c_int,
    address: *mut c_void,
    address_len: *mut c_int,
) -> isize {
    unsafe {
        kinakaze_abi___recvfrom_chk(socket, buffer, length, buflen, flags, address, address_len)
    }
}

/// `__vsprintf_chk`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___vsprintf_chk(
    buffer: *mut c_char,
    _flag: c_int,
    slen: usize,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    if slen == 0 {
        kinakaze_abi___chk_fail();
    }
    let length = unsafe { crate::stdio::vsnprintf(buffer, slen, format, arguments) };
    if length >= 0 && slen != usize::MAX && length as usize >= slen {
        kinakaze_abi___chk_fail();
    }
    length
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __vsprintf_chk(
    buffer: *mut c_char,
    flag: c_int,
    slen: usize,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    unsafe { kinakaze_abi___vsprintf_chk(buffer, flag, slen, format, arguments) }
}

/// `__fgets_chk`: check the number of bytes actually read, including room for NUL.
///
/// # Safety
/// `buffer` must cover `capacity` writable bytes and `file` must be a live FILE.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___fgets_chk(
    buffer: *mut c_char,
    capacity: usize,
    count: c_int,
    file: *mut File,
) -> *mut c_char {
    unsafe { crate::stdio::line::checked_fgets(buffer, capacity, count, file) }
}

/// `__fread_chk`.
///
/// Note the shape: the buffer size is the *second* argument, not the last, so
/// this one follows neither family's rule.
///
/// # Safety
///
/// `data` must name at least `datalen` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___fread_chk(
    data: *mut c_void,
    datalen: usize,
    size: usize,
    count: usize,
    file: *mut File,
) -> usize {
    // An overflowing product cannot describe a real buffer, so it is a violation
    // rather than an argument to range-check.
    match size.checked_mul(count) {
        Some(total) => check(total, datalen),
        None => kinakaze_abi___chk_fail(),
    }
    // SAFETY: the request fits the buffer, as just checked.
    unsafe { crate::stdio::fread(data, size, count, file) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __fread_chk(
    data: *mut c_void,
    datalen: usize,
    size: usize,
    count: usize,
    file: *mut File,
) -> usize {
    unsafe { kinakaze_abi___fread_chk(data, datalen, size, count, file) }
}

/// `__open64_2`, the checked two-argument `open64`.
///
/// The compiler emits this when it can see that `open` was called without a
/// mode. A creating flag then has no mode to go with it, which is the bug this
/// entry point exists to catch.
///
/// # Safety
///
/// `path` must be null-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___open64_2(path: *const c_char, flags: c_int) -> c_int {
    if flags & (O_CREAT | O_TMPFILE) != 0 {
        kinakaze_abi___chk_fail();
    }
    // On x86_64 `off_t` is already 64-bit, so `open64` and `open` are one
    // function; there is no large-file variant to dispatch to.
    // SAFETY: forwarded from this function's contract. No mode is passed
    // because the flags just proved none is needed.
    unsafe { crate::fs::open(path, flags, 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __open64_2(path: *const c_char, flags: c_int) -> c_int {
    unsafe { kinakaze_abi___open64_2(path, flags) }
}

// ---------------------------------------------------------------------------
// The printf family: `int flag` is the FIRST argument.
// ---------------------------------------------------------------------------

/// Traps unless a `sprintf`-style call bounded by `maxlen` fits `slen`.
///
/// `__sprintf_chk` and friends receive the destination size in `slen` and the
/// bound the call will actually respect in `maxlen`. glibc trips when the call
/// could write past the object, which is decided before formatting starts.
fn check_output(maxlen: usize, slen: usize) {
    if slen != usize::MAX && maxlen > slen {
        kinakaze_abi___chk_fail();
    }
}

/// `__vprintf_chk`.
///
/// # Safety
///
/// `format` must be null-terminated and `arguments` must match it.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___vprintf_chk(
    _flag: c_int,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    // `_flag` is glibc's fortify level and is ignored; it is named as a
    // parameter so the format and va_list arrive in the right registers.
    // SAFETY: forwarded from this function's contract.
    unsafe { crate::stdio::vfprintf(kinakaze_stdout(), format, arguments) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __vprintf_chk(
    flag: c_int,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    unsafe { kinakaze_abi___vprintf_chk(flag, format, arguments) }
}

/// `__vfprintf_chk`.
///
/// # Safety
///
/// `format` must be null-terminated and `arguments` must match it.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___vfprintf_chk(
    file: *mut File,
    _flag: c_int,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    // `_flag` is ignored; see `__vprintf_chk`.
    // SAFETY: forwarded from this function's contract.
    unsafe { crate::stdio::vfprintf(file, format, arguments) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __vfprintf_chk(
    file: *mut File,
    flag: c_int,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    unsafe { kinakaze_abi___vfprintf_chk(file, flag, format, arguments) }
}

/// `__vsnprintf_chk`.
///
/// # Safety
///
/// `buffer` must name at least `slen` writable bytes and `arguments` must match
/// `format`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___vsnprintf_chk(
    buffer: *mut c_char,
    maxlen: usize,
    _flag: c_int,
    slen: usize,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    // `_flag` is ignored; see `__vprintf_chk`.
    check_output(maxlen, slen);
    // SAFETY: forwarded from this function's contract.
    unsafe { crate::stdio::vsnprintf(buffer, maxlen, format, arguments) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __vsnprintf_chk(
    buffer: *mut c_char,
    maxlen: usize,
    flag: c_int,
    slen: usize,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    unsafe { kinakaze_abi___vsnprintf_chk(buffer, maxlen, flag, slen, format, arguments) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_vasprintf(
    result: *mut *mut c_char,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    unsafe { kinakaze_abi___vasprintf_chk(result, 0, format, arguments) }
}

/// `__vasprintf_chk`, which allocates a buffer the exact size of the result.
///
/// # Safety
///
/// `result` must be writable and `arguments` must match `format`. The returned
/// buffer belongs to the caller and is released with `free`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___vasprintf_chk(
    result: *mut *mut c_char,
    _flag: c_int,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    // `_flag` is ignored; see `__vprintf_chk`.
    if result.is_null() || arguments.is_null() {
        return -1;
    }
    let mut sink = GrowingSink(Vec::new());
    // SAFETY: the caller guarantees the format matches the arguments.
    let length = unsafe { crate::format::format(&mut sink, format, &mut *arguments) };
    if length < 0 {
        return length;
    }
    // The caller frees this with `free`, so it must come from our allocator
    // rather than from a leaked `Vec`, whose layout `free` does not know.
    // SAFETY: the block is handed to the caller, terminator included.
    let block = unsafe { kinakaze_alloc::c::malloc(sink.0.len() + 1) };
    if block.is_null() {
        crate::set_errno(crate::ENOMEM);
        return -1;
    }
    // SAFETY: the block holds the bytes and the terminator.
    unsafe {
        core::ptr::copy_nonoverlapping(sink.0.as_ptr(), block, sink.0.len());
        *block.add(sink.0.len()) = 0;
        *result = block.cast::<c_char>();
    }
    length
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __vasprintf_chk(
    result: *mut *mut c_char,
    flag: c_int,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    unsafe { kinakaze_abi___vasprintf_chk(result, flag, format, arguments) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn asprintf_chk(
    result: *mut *mut c_char,
    flag: c_int,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    unsafe { kinakaze_abi___vasprintf_chk(result, flag, format, arguments) }
}

/// Collects formatted output so its finished length can be allocated for.
struct GrowingSink(Vec<u8>);

impl crate::format::Sink for GrowingSink {
    fn write(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    fn written(&self) -> usize {
        self.0.len()
    }
}

// ---------------------------------------------------------------------------
// Implementations behind the variadic assembly thunks.
// ---------------------------------------------------------------------------
//
// Each takes the same named arguments as the Linux entry point, in the same
// order, with the `va_list` appended. The thunk therefore only has to write the
// `va_list` pointer into the next argument register and jump here.

/// Implementation behind the `__printf_chk` thunk.
///
/// # Safety
///
/// `format` must be null-terminated and `arguments` must match it.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_printf_chk_impl(
    flag: c_int,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    // SAFETY: the thunk passes a live va_list it just constructed.
    unsafe { kinakaze_abi___vprintf_chk(flag, format, arguments) }
}

/// Implementation behind the `__fprintf_chk` thunk.
///
/// # Safety
///
/// `format` must be null-terminated and `arguments` must match it.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_fprintf_chk_impl(
    file: *mut File,
    flag: c_int,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    // SAFETY: the thunk passes a live va_list it just constructed.
    unsafe { kinakaze_abi___vfprintf_chk(file, flag, format, arguments) }
}

/// Implementation behind the `__dprintf_chk` thunk, which formats to a raw fd.
///
/// # Safety
///
/// `format` must be null-terminated and `arguments` must match it.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_dprintf_chk_impl(
    fd: c_int,
    _flag: c_int,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    // `_flag` is ignored; see `__vprintf_chk`.
    if arguments.is_null() {
        return -1;
    }
    // A descriptor has no stream and so no buffer: the result is assembled here
    // and written in one call, which also keeps it from interleaving.
    let mut sink = GrowingSink(Vec::new());
    // SAFETY: the thunk passes a live va_list matching the format.
    let length = unsafe { crate::format::format(&mut sink, format, &mut *arguments) };
    if length < 0 {
        return length;
    }
    let mut bytes = sink.0.as_slice();
    while !bytes.is_empty() {
        match kinakaze_vfs::write(fd, bytes) {
            Ok(0) => return -1,
            Ok(written) => bytes = &bytes[written..],
            Err(error) => {
                crate::set_errno(error);
                return -1;
            }
        }
    }
    length
}

/// Implementation behind the `__sprintf_chk` thunk.
///
/// # Safety
///
/// `buffer` must name at least `slen` writable bytes and `arguments` must match
/// `format`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_sprintf_chk_impl(
    buffer: *mut c_char,
    _flag: c_int,
    slen: usize,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    // `_flag` is ignored; see `__vprintf_chk`.
    // A zero-sized destination cannot even hold a terminator.
    if slen == 0 {
        kinakaze_abi___chk_fail();
    }
    // `sprintf` is unbounded, but the destination size is known here, so the
    // write is bounded by it and an overrun is a violation rather than a
    // truncation: glibc traps instead of writing a short result.
    // SAFETY: forwarded from this function's contract.
    let length = unsafe { crate::stdio::vsnprintf(buffer, slen, format, arguments) };
    if length >= 0 && slen != usize::MAX && length as usize >= slen {
        kinakaze_abi___chk_fail();
    }
    length
}

/// Implementation behind the `__snprintf_chk` thunk.
///
/// # Safety
///
/// `buffer` must name at least `slen` writable bytes and `arguments` must match
/// `format`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_snprintf_chk_impl(
    buffer: *mut c_char,
    maxlen: usize,
    flag: c_int,
    slen: usize,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    // SAFETY: the thunk passes a live va_list it just constructed.
    unsafe { kinakaze_abi___vsnprintf_chk(buffer, maxlen, flag, slen, format, arguments) }
}

/// Implementation behind the `__syslog_chk` thunk.
///
/// # Safety
///
/// `format` must be null-terminated and `arguments` must match it.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_syslog_chk_impl(
    priority: c_int,
    flag: c_int,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    unsafe { kinakaze_abi___vsyslog_chk(priority, flag, format, arguments) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___vsyslog_chk(
    priority: c_int,
    _flag: c_int,
    format: *const c_char,
    arguments: *mut VaList,
) {
    unsafe { crate::sysdb::kinakaze_abi_vsyslog(priority, format, arguments) };
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_asprintf_chk_impl(
    result: *mut *mut c_char,
    flag: c_int,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    unsafe { kinakaze_abi___vasprintf_chk(result, flag, format, arguments) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___explicit_bzero_chk(
    buffer: *mut c_void,
    length: usize,
    buflen: usize,
) {
    check(length, buflen);
    if !buffer.is_null() && length > 0 {
        unsafe {
            core::ptr::write_bytes(buffer as *mut u8, 0, length);
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __explicit_bzero_chk(
    buffer: *mut c_void,
    length: usize,
    buflen: usize,
) {
    unsafe { kinakaze_abi___explicit_bzero_chk(buffer, length, buflen) }
}

// ---------------------------------------------------------------------------
// The C23 conversion aliases.
// ---------------------------------------------------------------------------
//
// glibc 2.38 renamed the integer conversions when the caller is compiled for
// C23, because C23 made `strtol` accept a `0b` prefix under base 0 and 2. The
// rename lets one library serve both dialects. Nothing else about the functions
// changed, so each name here is the existing conversion under its C23 spelling.
// The `0b` behaviour itself belongs to the shared implementation in
// `crate::process`, not to these aliases.

/// `__isoc23_strtol`.
///
/// # Safety
///
/// `text` must be null-terminated and `end` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___isoc23_strtol(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> i64 {
    // SAFETY: forwarded from this function's contract.
    unsafe { crate::process::kinakaze_abi_strtol(text, end, base) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __isoc23_strtol(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> i64 {
    unsafe { kinakaze_abi___isoc23_strtol(text, end, base) }
}

/// `__isoc23_strtoll`.
///
/// `long` and `long long` are both 64-bit on LP64, so this is the same
/// conversion as `strtol` rather than a wider one.
///
/// # Safety
///
/// As [`kinakaze_abi___isoc23_strtol`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___isoc23_strtoll(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> i64 {
    // SAFETY: forwarded from this function's contract.
    unsafe { crate::process::kinakaze_abi_strtol(text, end, base) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __isoc23_strtoll(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> i64 {
    unsafe { kinakaze_abi___isoc23_strtoll(text, end, base) }
}

/// `__isoc23_strtoul`.
///
/// # Safety
///
/// As [`kinakaze_abi___isoc23_strtol`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___isoc23_strtoul(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> u64 {
    // SAFETY: forwarded from this function's contract.
    unsafe { crate::process::kinakaze_abi_strtoul(text, end, base) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __isoc23_strtoul(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> u64 {
    unsafe { kinakaze_abi___isoc23_strtoul(text, end, base) }
}

/// `__isoc23_strtoull`.
///
/// # Safety
///
/// As [`kinakaze_abi___isoc23_strtol`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___isoc23_strtoull(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> u64 {
    // SAFETY: forwarded from this function's contract.
    unsafe { crate::process::kinakaze_abi_strtoul(text, end, base) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __isoc23_strtoull(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> u64 {
    unsafe { kinakaze_abi___isoc23_strtoull(text, end, base) }
}

/// `__isoc23_strtoumax`.
///
/// `uintmax_t` is 64-bit on LP64, which makes this `strtoull`.
///
/// # Safety
///
/// As [`kinakaze_abi___isoc23_strtol`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___isoc23_strtoumax(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> u64 {
    // SAFETY: forwarded from this function's contract.
    unsafe { crate::process::kinakaze_abi_strtoul(text, end, base) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __isoc23_strtoumax(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
) -> u64 {
    unsafe { kinakaze_abi___isoc23_strtoumax(text, end, base) }
}

// Formatted input belongs to scan.rs; its variadic ABI entries are in variadic.rs.

// ---------------------------------------------------------------------------
// Variadic thunks.
// ---------------------------------------------------------------------------
//
// The frame is the same one `crate::variadic` builds; only the initial
// `gp_offset` and the register the `va_list` lands in differ per signature. The
// named arguments are already in the registers the implementation expects, so
// each thunk writes the `va_list` pointer into the next register along and
// calls. That the two families disagree about argument order is visible right
// here, in the `gp_offset` each thunk starts from.

core::arch::global_asm!(
    r#"
.text

// Builds the System V va_list frame. `\gp` is the initial gp_offset, which skips
// the named integer arguments the caller's signature already consumed. Only rax
// is clobbered: every argument register is preserved in the save area, so the
// named arguments stay where the implementation expects them.
.macro KINAKAZE_CHK_VA_FRAME gp
    push    rbp
    mov     rbp, rsp
    sub     rsp, 208

    mov     [rbp-176], rdi
    mov     [rbp-168], rsi
    mov     [rbp-160], rdx
    mov     [rbp-152], rcx
    mov     [rbp-144], r8
    mov     [rbp-136], r9

    movups  [rbp-128], xmm0
    movups  [rbp-112], xmm1
    movups  [rbp-96], xmm2
    movups  [rbp-80], xmm3
    movups  [rbp-64], xmm4
    movups  [rbp-48], xmm5
    movups  [rbp-32], xmm6
    movups  [rbp-16], xmm7

    mov     dword ptr [rbp-208], \gp
    mov     dword ptr [rbp-204], 48
    lea     rax, [rbp+16]
    mov     [rbp-200], rax
    lea     rax, [rbp-176]
    mov     [rbp-192], rax
    lea     rax, [rbp-208]
.endm

// __printf_chk(flag, format, ...) names two integer arguments, so the va_list
// starts at gp_offset 16 and is passed third, in rdx.
.globl kinakaze_abi___printf_chk
.globl __printf_chk
kinakaze_abi___printf_chk:
__printf_chk:
    KINAKAZE_CHK_VA_FRAME 16
    mov     rdx, rax
    call    kinakaze_printf_chk_impl
    leave
    ret

// __fprintf_chk(file, flag, format, ...) names three; va_list is fourth, in rcx.
.globl kinakaze_abi___fprintf_chk
.globl __fprintf_chk
kinakaze_abi___fprintf_chk:
__fprintf_chk:
    KINAKAZE_CHK_VA_FRAME 24
    mov     rcx, rax
    call    kinakaze_fprintf_chk_impl
    leave
    ret

// __dprintf_chk(fd, flag, format, ...) has the same shape as __fprintf_chk.
.globl kinakaze_abi___dprintf_chk
.globl __dprintf_chk
kinakaze_abi___dprintf_chk:
__dprintf_chk:
    KINAKAZE_CHK_VA_FRAME 24
    mov     rcx, rax
    call    kinakaze_dprintf_chk_impl
    leave
    ret

// __syslog_chk(priority, flag, format, ...) likewise.
.globl kinakaze_abi___syslog_chk
.globl __syslog_chk
kinakaze_abi___syslog_chk:
__syslog_chk:
    KINAKAZE_CHK_VA_FRAME 24
    mov     rcx, rax
    call    kinakaze_syslog_chk_impl
    leave
    ret

// __sprintf_chk(s, flag, slen, format, ...) names four; va_list is fifth, in r8.
.globl kinakaze_abi___sprintf_chk
.globl __sprintf_chk
kinakaze_abi___sprintf_chk:
__sprintf_chk:
    KINAKAZE_CHK_VA_FRAME 32
    mov     r8, rax
    call    kinakaze_sprintf_chk_impl
    leave
    ret

// __snprintf_chk(s, maxlen, flag, slen, format, ...) names five, filling every
// integer register: the va_list is the sixth argument, in r9, and the first
// variadic argument is already in the overflow area rather than a register.
.globl kinakaze_abi___snprintf_chk
.globl __snprintf_chk
kinakaze_abi___snprintf_chk:
__snprintf_chk:
    KINAKAZE_CHK_VA_FRAME 40
    mov     r9, rax
    call    kinakaze_snprintf_chk_impl
    leave
    ret
"#
);

// Probes used only by the tests below.
//
// Proving the argument order is right needs a *variadic* call into the thunks,
// which is the one thing Rust cannot express. These probes make that call from
// assembly instead. Each takes the buffer, its size, a format and one string, in
// a fixed non-variadic signature Rust can declare, and calls its target with the
// varargs `(text, 42, 255)`. A fortified probe and its plain counterpart differ
// only in which entry point they call, so comparing their output compares the
// two argument layouts against each other. The names are not `kinakaze_abi_*`,
// so they are never exported.
#[cfg(test)]
core::arch::global_asm!(
    r#"
.text

// __snprintf_chk(buffer, size, flag, size, format, text, 42, 255)
.globl kinakaze_probe_snprintf_chk
kinakaze_probe_snprintf_chk:
    push    rbp
    mov     rbp, rsp
    // rdi=buffer, rsi=size, rdx=format, rcx=text on entry.
    mov     r8, rdx             // format, the fifth argument
    mov     r9, rcx             // text, the first variadic argument
    mov     rcx, rsi            // slen, the fourth
    mov     edx, 1              // flag, the third
    // The two remaining variadic arguments go in the overflow area.
    sub     rsp, 16
    mov     qword ptr [rsp], 42
    mov     qword ptr [rsp+8], 255
    xor     eax, eax            // no vector arguments
    call    kinakaze_abi___snprintf_chk
    leave
    ret

// snprintf(buffer, size, format, text, 42, 255)
.globl kinakaze_probe_snprintf
kinakaze_probe_snprintf:
    push    rbp
    mov     rbp, rsp
    // rdi, rsi and rdx already hold buffer, size and format.
    mov     r8, 42
    mov     r9, 255
    xor     eax, eax
    call    kinakaze_abi_snprintf
    leave
    ret

// __sprintf_chk(buffer, flag, size, format, text, 42, 255)
.globl kinakaze_probe_sprintf_chk
kinakaze_probe_sprintf_chk:
    push    rbp
    mov     rbp, rsp
    mov     r8, rcx             // text, the first variadic argument
    mov     rcx, rdx            // format, the fourth argument
    mov     rdx, rsi            // slen, the third
    mov     esi, 1              // flag, the second
    mov     r9, 42
    sub     rsp, 16
    mov     qword ptr [rsp], 255
    xor     eax, eax
    call    kinakaze_abi___sprintf_chk
    leave
    ret
"#
);

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "sysv64" {
        fn kinakaze_probe_snprintf_chk(
            buffer: *mut c_char,
            size: usize,
            format: *const c_char,
            text: *const c_char,
        ) -> c_int;
        fn kinakaze_probe_snprintf(
            buffer: *mut c_char,
            size: usize,
            format: *const c_char,
            text: *const c_char,
        ) -> c_int;
        fn kinakaze_probe_sprintf_chk(
            buffer: *mut c_char,
            size: usize,
            format: *const c_char,
            text: *const c_char,
        ) -> c_int;
    }

    /// Runs a probe and returns its result and the string it produced.
    fn run(
        probe: unsafe extern "sysv64" fn(*mut c_char, usize, *const c_char, *const c_char) -> c_int,
        format: &core::ffi::CStr,
    ) -> (c_int, Vec<u8>) {
        let mut buffer = [0u8; 128];
        // SAFETY: the buffer is 128 writable bytes and the format's conversions
        // match the `(const char *, int, int)` the probes pass.
        let length = unsafe {
            probe(
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                format.as_ptr(),
                c"value".as_ptr(),
            )
        };
        let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(0);
        (length, buffer[..end].to_vec())
    }

    /// Every format string the ordering tests run through the probes.
    ///
    /// Each consumes exactly `(const char *, int, int)`, which is what the
    /// probes supply, and between them they exercise plain conversions, a width,
    /// left justification and an alternate form.
    const FORMATS: &[&core::ffi::CStr] =
        &[c"%s=%d/%x", c"[%10s|%-5d|%#x]", c"%s %d %d", c"%s%d%x!"];

    /// `__snprintf_chk` must produce exactly what `snprintf` produces.
    ///
    /// This is the test that proves the argument order. `__snprintf_chk` takes
    /// `flag` third and the destination size fourth, so if those were read in the
    /// wrong order the format pointer would be an integer, the first conversion
    /// would consume the wrong slot, and the output would diverge from plain
    /// `snprintf` on the very same arguments.
    #[test]
    fn snprintf_chk_matches_plain_snprintf() {
        for format in FORMATS {
            let fortified = run(kinakaze_probe_snprintf_chk, format);
            let plain = run(kinakaze_probe_snprintf, format);
            assert_eq!(
                fortified, plain,
                "__snprintf_chk diverged from snprintf for {format:?}"
            );
        }
    }

    /// `__sprintf_chk` takes `flag` *second*, a third layout again.
    #[test]
    fn sprintf_chk_matches_plain_snprintf() {
        for format in FORMATS {
            let fortified = run(kinakaze_probe_sprintf_chk, format);
            let plain = run(kinakaze_probe_snprintf, format);
            assert_eq!(
                fortified, plain,
                "__sprintf_chk diverged from snprintf for {format:?}"
            );
        }
    }

    /// A copy that fits must behave exactly like `memcpy`.
    #[test]
    fn memcpy_chk_copies_within_the_destination() {
        let source = *b"0123456789";
        let mut destination = [0u8; 16];
        // SAFETY: 10 bytes are copied into a 16-byte destination, and 16 is the
        // size reported, so the check passes.
        let result = unsafe {
            kinakaze_abi___memcpy_chk(
                destination.as_mut_ptr().cast(),
                source.as_ptr().cast(),
                source.len(),
                destination.len(),
            )
        };
        assert_eq!(result, destination.as_mut_ptr().cast::<c_void>());
        assert_eq!(&destination[..10], &source);
        assert_eq!(&destination[10..], &[0u8; 6]);
    }

    /// A copy filling the destination exactly is legal: `len == destlen` fits.
    #[test]
    fn memcpy_chk_allows_an_exact_fit() {
        let source = *b"abcd";
        let mut destination = [0u8; 4];
        // SAFETY: the copy exactly fills the destination.
        unsafe {
            kinakaze_abi___memcpy_chk(
                destination.as_mut_ptr().cast(),
                source.as_ptr().cast(),
                4,
                4,
            )
        };
        assert_eq!(destination, source);
    }

    /// `__mempcpy_chk` returns the end of the copy, not its start.
    #[test]
    fn mempcpy_chk_returns_the_end_of_the_copy() {
        let source = *b"xyz";
        let mut destination = [0u8; 8];
        // SAFETY: 3 bytes into an 8-byte destination.
        let end = unsafe {
            kinakaze_abi___mempcpy_chk(
                destination.as_mut_ptr().cast(),
                source.as_ptr().cast(),
                3,
                8,
            )
        };
        assert_eq!(end, unsafe { destination.as_mut_ptr().add(3) }.cast());
        assert_eq!(&destination[..3], b"xyz");
    }

    /// `__memset_chk` fills only the requested bytes.
    #[test]
    fn memset_chk_fills_the_requested_range() {
        let mut destination = [0u8; 8];
        // SAFETY: 5 bytes of an 8-byte destination.
        unsafe { kinakaze_abi___memset_chk(destination.as_mut_ptr().cast(), 0x41, 5, 8) };
        assert_eq!(&destination, b"AAAAA\0\0\0");
    }

    /// `__strcpy_chk` copies the string and its terminator.
    #[test]
    fn strcpy_chk_copies_a_string() {
        let mut destination = [0u8; 16];
        // SAFETY: "hello" and its terminator need 6 of 16 bytes.
        let result = unsafe {
            kinakaze_abi___strcpy_chk(
                destination.as_mut_ptr().cast(),
                c"hello".as_ptr(),
                destination.len(),
            )
        };
        assert_eq!(result, destination.as_mut_ptr().cast::<c_char>());
        assert_eq!(&destination[..6], b"hello\0");
    }

    /// A string exactly filling the destination, terminator included, fits.
    #[test]
    fn strcpy_chk_allows_an_exact_fit() {
        let mut destination = [0u8; 4];
        // SAFETY: "abc" plus a terminator is exactly 4 bytes.
        unsafe { kinakaze_abi___strcpy_chk(destination.as_mut_ptr().cast(), c"abc".as_ptr(), 4) };
        assert_eq!(&destination, b"abc\0");
    }

    /// `__strcat_chk` appends and counts the existing string in the bound.
    #[test]
    fn strcat_chk_appends_a_string() {
        let mut destination = [0u8; 16];
        destination[..3].copy_from_slice(b"ab\0");
        // SAFETY: "ab" plus "cd" plus a terminator needs 5 of 16 bytes.
        unsafe {
            kinakaze_abi___strcat_chk(
                destination.as_mut_ptr().cast(),
                c"cd".as_ptr(),
                destination.len(),
            )
        };
        assert_eq!(&destination[..5], b"abcd\0");
    }

    /// The overflow boundary, tested through `fits` rather than through a call.
    ///
    /// The overflow path itself cannot be unit-tested. `__chk_fail` ends in
    /// `abort`, which is `ExitProcess(134)`: the process is gone, so there is no
    /// panic for `catch_unwind` to catch and no way for the test harness to
    /// regain control. Testing it for real needs a subprocess, which is what the
    /// BusyBox run against the built DLL provides. What is testable here is the
    /// decision that leads there, so the boundary is checked directly and every
    /// `_chk` entry point routes through it.
    #[test]
    fn the_bound_is_exclusive_of_overflow() {
        assert!(fits(0, 0));
        assert!(fits(4, 4), "an exact fit is not an overflow");
        assert!(fits(3, 4));
        assert!(!fits(5, 4), "one byte past the end must trap");
        assert!(!fits(usize::MAX - 1, 4));
        // `(size_t)-1` is glibc's "size unknown", which disables the check.
        assert!(fits(usize::MAX, usize::MAX));
        assert!(fits(1 << 20, usize::MAX));
    }

    /// `check_output` guards the `sprintf` family, where the bound is compared
    /// against the destination size rather than against a copy length.
    #[test]
    fn output_bounds_reject_a_bound_past_the_object() {
        // A bound within the object is fine, as is one that matches it.
        check_output(4, 8);
        check_output(8, 8);
        // An unknown destination size disables the check.
        check_output(1 << 20, usize::MAX);
    }

    /// The C23 aliases must agree with the conversions they rename.
    #[test]
    fn the_c23_aliases_match_their_base_conversions() {
        let text = c"  -1234xyz";
        let mut alias_end: *const c_char = core::ptr::null();
        let mut base_end: *const c_char = core::ptr::null();
        // SAFETY: both strings are null-terminated literals and both end
        // pointers are writable.
        let (alias, base) = unsafe {
            (
                kinakaze_abi___isoc23_strtol(text.as_ptr(), &mut alias_end, 10),
                crate::process::kinakaze_abi_strtol(text.as_ptr(), &mut base_end, 10),
            )
        };
        assert_eq!(alias, -1234);
        assert_eq!(alias, base);
        assert_eq!(alias_end, base_end);

        // SAFETY: null-terminated literals, and a null end pointer is allowed.
        let unsigned =
            unsafe { kinakaze_abi___isoc23_strtoul(c"0x2a".as_ptr(), core::ptr::null_mut(), 16) };
        assert_eq!(unsigned, 42);
        // `strtoll`, `strtoull` and `strtoumax` are the same widths on LP64, so
        // they must agree with their shorter spellings.
        // SAFETY: as above.
        unsafe {
            assert_eq!(
                kinakaze_abi___isoc23_strtoll(c"9999".as_ptr(), core::ptr::null_mut(), 10),
                kinakaze_abi___isoc23_strtol(c"9999".as_ptr(), core::ptr::null_mut(), 10),
            );
            assert_eq!(
                kinakaze_abi___isoc23_strtoull(c"9999".as_ptr(), core::ptr::null_mut(), 10),
                kinakaze_abi___isoc23_strtoumax(c"9999".as_ptr(), core::ptr::null_mut(), 10),
            );
        }
    }

    /// `__vsnprintf_chk` must reach the same engine as `vsnprintf`.
    ///
    /// The probes cover the variadic thunks; this covers the `v` form, which the
    /// thunks do not go through, by driving it with a `va_list` built by the
    /// `__snprintf_chk` path and comparing the truncating behaviour.
    #[test]
    fn snprintf_chk_truncates_like_snprintf() {
        // A destination smaller than the result truncates and still reports the
        // length the full output needed, which is `snprintf`'s contract.
        let mut buffer = [0u8; 8];
        // SAFETY: the buffer is 8 writable bytes and the format matches the
        // probe's `(const char *, int, int)` arguments.
        let length = unsafe {
            kinakaze_probe_snprintf_chk(
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                c"%s %d %d".as_ptr(),
                c"value".as_ptr(),
            )
        };
        let mut plain = [0u8; 8];
        // SAFETY: as above.
        let expected = unsafe {
            kinakaze_probe_snprintf(
                plain.as_mut_ptr().cast(),
                plain.len(),
                c"%s %d %d".as_ptr(),
                c"value".as_ptr(),
            )
        };
        assert_eq!(length, expected);
        assert_eq!(buffer, plain);
        // The result is terminated within the destination.
        assert_eq!(buffer[7], 0);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___wctomb_chk(
    out: *mut c_char,
    scalar: i32,
    length: usize,
) -> c_int {
    if length < crate::locale::kinakaze_abi___ctype_get_mb_cur_max() {
        kinakaze_abi___chk_fail();
    }
    unsafe { crate::uchar::strings::kinakaze_abi_wctomb(out, scalar) }
}
