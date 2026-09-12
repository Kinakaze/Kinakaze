//! String and memory C ABI exports.
//!
//! These are the pure-computation half of libc: no host calls, no descriptor
//! table, just pointer arithmetic. Each function follows the C contract
//! exactly, including the parts that are easy to get subtly wrong (`strncpy`
//! not terminating, `strncat` always terminating, `memmove` tolerating
//! overlap).

use core::ffi::{c_char, c_int, c_void};
use core::ptr;

#[cfg(target_arch = "x86_64")]
mod simd;

/// Swap each complete byte pair. A negative count and an odd trailing byte
/// transfer nothing; reading a pair before writing also permits in-place use.
/// # Safety
/// Both regions must hold `count.max(0)` bytes; partial overlap is not supported.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_swab(
    source: *const c_void,
    destination: *mut c_void,
    count: isize,
) {
    for i in 0..count.max(0) as usize / 2 {
        let pair = unsafe { source.cast::<u16>().add(i).read_unaligned() };
        unsafe {
            destination
                .cast::<u16>()
                .add(i)
                .write_unaligned(pair.swap_bytes());
        }
    }
}

/// Returns the length of a null-terminated string.
///
/// # Safety
///
/// `text` must point to a null-terminated string.
pub unsafe extern "sysv64" fn strlen(text: *const c_char) -> usize {
    let mut length = 0;
    // SAFETY: the caller guarantees a terminator is present.
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    length
}

/// Returns the length of `text`, examining at most `limit` bytes.
///
/// # Safety
///
/// `text` must be readable for `limit` bytes or up to its terminator.
pub unsafe extern "sysv64" fn strnlen(text: *const c_char, limit: usize) -> usize {
    let mut length = 0;
    // SAFETY: the scan stops at `limit` even without a terminator.
    while length < limit && unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    length
}

/// Copies bytes between non-overlapping regions.
///
/// # Safety
///
/// `destination` and `source` must be valid for `count` bytes and must not
/// overlap.
pub unsafe extern "sysv64" fn memcpy(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
) -> *mut c_void {
    // SAFETY: the caller guarantees both regions and non-overlap.
    unsafe { ptr::copy_nonoverlapping(source.cast::<u8>(), destination.cast::<u8>(), count) };
    destination
}

/// Copies bytes, tolerating overlap.
///
/// # Safety
///
/// Both pointers must be valid for `count` bytes.
pub unsafe extern "sysv64" fn memmove(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
) -> *mut c_void {
    // SAFETY: `copy` is the overlap-tolerant form, which is what memmove means.
    unsafe { ptr::copy(source.cast::<u8>(), destination.cast::<u8>(), count) };
    destination
}

/// Fills a region with a byte value.
///
/// # Safety
///
/// `destination` must be writable for `count` bytes.
pub unsafe extern "sysv64" fn memset(
    destination: *mut c_void,
    value: c_int,
    count: usize,
) -> *mut c_void {
    // C converts the int to unsigned char before storing it.
    // SAFETY: the caller guarantees the region is writable.
    unsafe { ptr::write_bytes(destination.cast::<u8>(), value as u8, count) };
    destination
}

/// Compares two regions byte by byte.
///
/// # Safety
///
/// Both pointers must be readable for `count` bytes.
pub unsafe extern "sysv64" fn memcmp(
    left: *const c_void,
    right: *const c_void,
    count: usize,
) -> c_int {
    #[cfg(target_arch = "x86_64")]
    {
        // Compare full vectors only; the first differing unsigned byte decides
        // the result, and the tail never reads past the caller's exact count.
        return match unsafe { simd::first_mismatch(left.cast(), right.cast(), count) } {
            Some(index) => unsafe {
                *left.cast::<u8>().add(index) as c_int - *right.cast::<u8>().add(index) as c_int
            },
            None => 0,
        };
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        // SAFETY: the caller guarantees both regions.
        let left = unsafe { core::slice::from_raw_parts(left.cast::<u8>(), count) };
        // SAFETY: the caller guarantees both regions.
        let right = unsafe { core::slice::from_raw_parts(right.cast::<u8>(), count) };
        for (left, right) in left.iter().zip(right) {
            if left != right {
                // The result is the difference of the bytes as unsigned char.
                return *left as c_int - *right as c_int;
            }
        }
        0
    }
}

/// Finds the first occurrence of a byte.
///
/// # Safety
///
/// `haystack` must be readable for `count` bytes.
pub unsafe extern "sysv64" fn memchr(
    haystack: *const c_void,
    needle: c_int,
    count: usize,
) -> *mut c_void {
    let needle = needle as u8;
    #[cfg(target_arch = "x86_64")]
    let found = unsafe { simd::find_byte(haystack.cast(), needle, count) };
    #[cfg(not(target_arch = "x86_64"))]
    let found = {
        // SAFETY: the caller guarantees the region.
        let bytes = unsafe { core::slice::from_raw_parts(haystack.cast::<u8>(), count) };
        bytes.iter().position(|byte| *byte == needle)
    };
    match found {
        // SAFETY: the index came from within the region.
        Some(index) => unsafe { haystack.cast::<u8>().add(index) as *mut c_void },
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn rawmemchr(haystack: *const c_void, needle: c_int) -> *mut c_void {
    let needle = needle as u8;
    let mut ptr = haystack.cast::<u8>();
    unsafe {
        while *ptr != needle {
            ptr = ptr.add(1);
        }
        ptr as *mut c_void
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_rawmemchr(
    haystack: *const c_void,
    needle: c_int,
) -> *mut c_void {
    unsafe { rawmemchr(haystack, needle) }
}

/// Copies a null-terminated string, including its terminator.
///
/// # Safety
///
/// `source` must be null-terminated and `destination` must have room for the
/// string plus its terminator.
pub unsafe extern "sysv64" fn strcpy(
    destination: *mut c_char,
    source: *const c_char,
) -> *mut c_char {
    let mut index = 0;
    loop {
        // SAFETY: the caller guarantees a terminator and enough room.
        let byte = unsafe { *source.add(index) };
        // SAFETY: writing at most through the terminator.
        unsafe { *destination.add(index) = byte };
        if byte == 0 {
            return destination;
        }
        index += 1;
    }
}

/// Copies at most `limit` bytes, padding with NULs.
///
/// This deliberately does not guarantee termination: if `source` is at least
/// `limit` bytes long the result is unterminated, which is the C contract and a
/// classic source of bugs in calling code.
///
/// # Safety
///
/// `destination` must be writable for `limit` bytes.
pub unsafe extern "sysv64" fn strncpy(
    destination: *mut c_char,
    source: *const c_char,
    limit: usize,
) -> *mut c_char {
    let mut index = 0;
    // SAFETY: reads stop at the terminator, writes stay under `limit`.
    while index < limit && unsafe { *source.add(index) } != 0 {
        // SAFETY: `index` is below `limit`.
        unsafe { *destination.add(index) = *source.add(index) };
        index += 1;
    }
    // The remainder of the buffer is zero-filled.
    while index < limit {
        // SAFETY: `index` is below `limit`.
        unsafe { *destination.add(index) = 0 };
        index += 1;
    }
    destination
}

/// `stpcpy`
pub unsafe extern "sysv64" fn stpcpy(
    destination: *mut c_char,
    source: *const c_char,
) -> *mut c_char {
    let mut index = 0;
    loop {
        let byte = unsafe { *source.add(index) };
        unsafe { *destination.add(index) = byte };
        if byte == 0 {
            return unsafe { destination.add(index) };
        }
        index += 1;
    }
}

/// `stpncpy`
pub unsafe extern "sysv64" fn stpncpy(
    destination: *mut c_char,
    source: *const c_char,
    limit: usize,
) -> *mut c_char {
    let mut index = 0;
    while index < limit && unsafe { *source.add(index) } != 0 {
        unsafe { *destination.add(index) = *source.add(index) };
        index += 1;
    }
    let end = unsafe { destination.add(index) };
    while index < limit {
        unsafe { *destination.add(index) = 0 };
        index += 1;
    }
    end
}

/// Appends a string to the end of another.
///
/// # Safety
///
/// Both must be null-terminated and `destination` must have room for the
/// combined result plus its terminator.
pub unsafe extern "sysv64" fn strcat(
    destination: *mut c_char,
    source: *const c_char,
) -> *mut c_char {
    // SAFETY: the caller guarantees `destination` is null-terminated.
    let end = unsafe { strlen(destination) };
    // SAFETY: appending at the existing terminator, with room guaranteed.
    unsafe { strcpy(destination.add(end), source) };
    destination
}

/// Appends at most `limit` bytes and always terminates the result.
///
/// Unlike [`strncpy`], `strncat` always writes a terminator, so the buffer must
/// have room for `limit + 1` bytes past the existing contents.
///
/// # Safety
///
/// `destination` must be null-terminated with room for `limit + 1` more bytes.
pub unsafe extern "sysv64" fn strncat(
    destination: *mut c_char,
    source: *const c_char,
    limit: usize,
) -> *mut c_char {
    // SAFETY: the caller guarantees `destination` is null-terminated.
    let end = unsafe { strlen(destination) };
    let mut index = 0;
    // SAFETY: bounded by `limit` and the source terminator.
    while index < limit && unsafe { *source.add(index) } != 0 {
        // SAFETY: room for `limit + 1` bytes past `end` is guaranteed.
        unsafe { *destination.add(end + index) = *source.add(index) };
        index += 1;
    }
    // SAFETY: the terminator slot is the guaranteed `limit + 1`-th byte.
    unsafe { *destination.add(end + index) = 0 };
    destination
}

/// Compares two null-terminated strings.
///
/// # Safety
///
/// Both must be null-terminated.
pub unsafe extern "sysv64" fn strcmp(left: *const c_char, right: *const c_char) -> c_int {
    if left.is_null() && right.is_null() {
        return 0;
    }
    if left.is_null() {
        return -1;
    }
    if right.is_null() {
        return 1;
    }
    let mut index = 0;
    loop {
        let left_byte = unsafe { *left.add(index) } as u8;
        let right_byte = unsafe { *right.add(index) } as u8;
        if left_byte != right_byte {
            return left_byte as c_int - right_byte as c_int;
        }
        if left_byte == 0 {
            return 0;
        }
        index += 1;
    }
}

/// Compares at most `limit` bytes of two strings.
pub unsafe extern "sysv64" fn strncmp(
    left: *const c_char,
    right: *const c_char,
    limit: usize,
) -> c_int {
    if limit == 0 {
        return 0;
    }
    if left.is_null() && right.is_null() {
        return 0;
    }
    if left.is_null() {
        return -1;
    }
    if right.is_null() {
        return 1;
    }
    for index in 0..limit {
        let left_byte = unsafe { *left.add(index) } as u8;
        let right_byte = unsafe { *right.add(index) } as u8;
        if left_byte != right_byte {
            return left_byte as c_int - right_byte as c_int;
        }
        if left_byte == 0 {
            return 0;
        }
    }
    0
}

/// Finds the first occurrence of a character.
///
/// Searching for `\0` finds the terminator and returns a pointer to it, which C
/// specifies deliberately.
///
/// # Safety
///
/// `text` must be null-terminated.
pub unsafe extern "sysv64" fn strchr(text: *const c_char, needle: c_int) -> *mut c_char {
    let needle = needle as u8;
    let mut index = 0;
    loop {
        // SAFETY: the caller guarantees a terminator.
        let byte = unsafe { *text.add(index) } as u8;
        if byte == needle {
            // SAFETY: `index` is within the string.
            return unsafe { text.add(index) as *mut c_char };
        }
        if byte == 0 {
            return ptr::null_mut();
        }
        index += 1;
    }
}

/// Finds the last occurrence of a character.
///
/// # Safety
///
/// `text` must be null-terminated.
pub unsafe extern "sysv64" fn strrchr(text: *const c_char, needle: c_int) -> *mut c_char {
    let needle = needle as u8;
    let mut found = ptr::null_mut();
    let mut index = 0;
    loop {
        // SAFETY: the caller guarantees a terminator.
        let byte = unsafe { *text.add(index) } as u8;
        if byte == needle {
            // SAFETY: `index` is within the string.
            found = unsafe { text.add(index) as *mut c_char };
        }
        if byte == 0 {
            return found;
        }
        index += 1;
    }
}

/// Finds the first occurrence of a substring.
///
/// # Safety
///
/// Both must be null-terminated.
pub unsafe extern "sysv64" fn strstr(
    haystack: *const c_char,
    needle: *const c_char,
) -> *mut c_char {
    // SAFETY: the caller guarantees both terminators.
    let needle_length = unsafe { strlen(needle) };
    // An empty needle matches at the start of the haystack.
    if needle_length == 0 {
        return haystack as *mut c_char;
    }
    // SAFETY: the caller guarantees both terminators.
    let haystack_length = unsafe { strlen(haystack) };
    if needle_length > haystack_length {
        return ptr::null_mut();
    }
    for start in 0..=(haystack_length - needle_length) {
        // SAFETY: the window fits inside the haystack.
        let equal = unsafe { strncmp(haystack.add(start), needle, needle_length) } == 0;
        if equal {
            // SAFETY: `start` indexes within the haystack.
            return unsafe { haystack.add(start) as *mut c_char };
        }
    }
    ptr::null_mut()
}

/// Returns the length of the prefix consisting only of bytes in `accept`.
///
/// # Safety
///
/// Both must be null-terminated.
pub unsafe extern "sysv64" fn strspn(text: *const c_char, accept: *const c_char) -> usize {
    let mut length = 0;
    loop {
        // SAFETY: the caller guarantees a terminator.
        let byte = unsafe { *text.add(length) };
        // SAFETY: `accept` is null-terminated, and strchr never matches here
        // because a zero `byte` exits first.
        if byte == 0 || unsafe { strchr(accept, byte as c_int) }.is_null() {
            return length;
        }
        length += 1;
    }
}

/// Returns the length of the prefix containing no bytes from `reject`.
///
/// # Safety
///
/// Both must be null-terminated.
pub unsafe extern "sysv64" fn strcspn(text: *const c_char, reject: *const c_char) -> usize {
    let mut length = 0;
    loop {
        // SAFETY: the caller guarantees a terminator.
        let byte = unsafe { *text.add(length) };
        if byte == 0 {
            return length;
        }
        // SAFETY: `reject` is null-terminated.
        if !unsafe { strchr(reject, byte as c_int) }.is_null() {
            return length;
        }
        length += 1;
    }
}

/// Finds the first byte that appears in `accept`.
///
/// # Safety
///
/// Both must be null-terminated.
pub unsafe extern "sysv64" fn strpbrk(text: *const c_char, accept: *const c_char) -> *mut c_char {
    // SAFETY: both are null-terminated.
    let offset = unsafe { strcspn(text, accept) };
    // SAFETY: `offset` is at most the string length.
    if unsafe { *text.add(offset) } == 0 {
        ptr::null_mut()
    } else {
        // SAFETY: `offset` indexes a real byte within the string.
        unsafe { text.add(offset) as *mut c_char }
    }
}

/// Duplicates a string into a fresh allocation.
///
/// # Safety
///
/// `text` must be null-terminated. The result must be released with `free`.
pub unsafe extern "sysv64" fn strdup(text: *const c_char) -> *mut c_char {
    // SAFETY: the caller guarantees a terminator.
    let length = unsafe { strlen(text) };
    // SAFETY: request room for the string and its terminator.
    let copy = unsafe { kinakaze_alloc::c::malloc(length + 1) };
    if copy.is_null() {
        crate::set_errno(crate::ENOMEM);
        return ptr::null_mut();
    }
    // SAFETY: the allocation holds `length + 1` bytes.
    unsafe {
        ptr::copy_nonoverlapping(text.cast::<u8>(), copy, length);
        *copy.add(length) = 0;
    }
    copy.cast()
}

/// Duplicates at most `limit` bytes, always terminating the result.
///
/// # Safety
///
/// `text` must be readable for `limit` bytes or up to its terminator. The
/// result must be released with `free`.
pub unsafe extern "sysv64" fn strndup(text: *const c_char, limit: usize) -> *mut c_char {
    // SAFETY: bounded by `limit`.
    let length = unsafe { strnlen(text, limit) };
    // SAFETY: request room for the prefix and a terminator.
    let copy = unsafe { kinakaze_alloc::c::malloc(length + 1) };
    if copy.is_null() {
        crate::set_errno(crate::ENOMEM);
        return ptr::null_mut();
    }
    // SAFETY: the allocation holds `length + 1` bytes.
    unsafe {
        ptr::copy_nonoverlapping(text.cast::<u8>(), copy, length);
        *copy.add(length) = 0;
    }
    copy.cast()
}

/// Returns the message for an errno value.
///
/// Static NUL-terminated messages share one catalog. Unknown values return None
/// so each public ABI can implement its specified buffer and error behavior.
fn errno_message(error: c_int) -> Option<&'static str> {
    Some(match error {
        0 => "Success\0",
        kinakaze_vfs::EPERM => "Operation not permitted\0",
        kinakaze_vfs::ENOENT => "No such file or directory\0",
        kinakaze_vfs::EINTR => "Interrupted system call\0",
        kinakaze_vfs::EIO => "Input/output error\0",
        kinakaze_vfs::EBADF => "Bad file descriptor\0",
        kinakaze_vfs::EAGAIN => "Resource temporarily unavailable\0",
        kinakaze_vfs::ENOMEM => "Cannot allocate memory\0",
        kinakaze_vfs::EACCES => "Permission denied\0",
        kinakaze_vfs::EFAULT => "Bad address\0",
        kinakaze_vfs::EBUSY => "Device or resource busy\0",
        kinakaze_vfs::EEXIST => "File exists\0",
        kinakaze_vfs::EXDEV => "Invalid cross-device link\0",
        kinakaze_vfs::ENOTDIR => "Not a directory\0",
        kinakaze_vfs::EISDIR => "Is a directory\0",
        kinakaze_vfs::EINVAL => "Invalid argument\0",
        kinakaze_vfs::EMFILE => "Too many open files\0",
        kinakaze_vfs::ENOTTY => "Inappropriate ioctl for device\0",
        kinakaze_vfs::ENOSPC => "No space left on device\0",
        kinakaze_vfs::ESPIPE => "Illegal seek\0",
        kinakaze_vfs::EPIPE => "Broken pipe\0",
        kinakaze_vfs::ERANGE => "Numerical result out of range\0",
        kinakaze_vfs::ENAMETOOLONG => "File name too long\0",
        kinakaze_vfs::ENOSYS => "Function not implemented\0",
        kinakaze_vfs::ENOTEMPTY => "Directory not empty\0",
        kinakaze_vfs::ELOOP => "Too many levels of symbolic links\0",
        3 => "No such process\0",
        6 => "No such device or address\0",
        7 => "Argument list too long\0",
        8 => "Exec format error\0",
        10 => "No child processes\0",
        15 => "Block device required\0",
        19 => "No such device\0",
        23 => "Too many open files in system\0",
        26 => "Text file busy\0",
        27 => "File too large\0",
        30 => "Read-only file system\0",
        31 => "Too many links\0",
        33 => "Numerical argument out of domain\0",
        35 => "Resource deadlock avoided\0",
        37 => "No locks available\0",
        42 => "No message of desired type\0",
        43 => "Identifier removed\0",
        44 => "Channel number out of range\0",
        45 => "Level 2 not synchronized\0",
        46 => "Level 3 halted\0",
        47 => "Level 3 reset\0",
        48 => "Link number out of range\0",
        49 => "Protocol driver not attached\0",
        50 => "No CSI structure available\0",
        51 => "Level 2 halted\0",
        52 => "Invalid exchange\0",
        53 => "Invalid request descriptor\0",
        54 => "Exchange full\0",
        55 => "No anode\0",
        56 => "Invalid request code\0",
        57 => "Invalid slot\0",
        59 => "Bad font file format\0",
        60 => "Device not a stream\0",
        61 => "No data available\0",
        62 => "Timer expired\0",
        63 => "Out of streams resources\0",
        64 => "Machine is not on the network\0",
        65 => "Package not installed\0",
        66 => "Object is remote\0",
        67 => "Link has been severed\0",
        68 => "Advertise error\0",
        69 => "Srmount error\0",
        70 => "Communication error on send\0",
        71 => "Protocol error\0",
        72 => "Multihop attempted\0",
        73 => "RFS specific error\0",
        74 => "Bad message\0",
        75 => "Value too large for defined data type\0",
        76 => "Name not unique on network\0",
        77 => "File descriptor in bad state\0",
        78 => "Remote address changed\0",
        79 => "Cannot access a needed shared library\0",
        80 => "Accessing a corrupted shared library\0",
        81 => "Invalid library section in executable\0",
        82 => "Too many shared libraries\0",
        83 => "Cannot execute a shared library directly\0",
        84 => "Invalid multibyte or wide character\0",
        85 => "Interrupted system call should be restarted\0",
        86 => "Streams pipe error\0",
        87 => "Too many users\0",
        88 => "Socket operation on non-socket\0",
        89 => "Destination address required\0",
        90 => "Message too long\0",
        91 => "Protocol wrong type for socket\0",
        92 => "Protocol not available\0",
        93 => "Protocol not supported\0",
        94 => "Socket type not supported\0",
        95 => "Operation not supported\0",
        96 => "Protocol family not supported\0",
        97 => "Address family not supported by protocol\0",
        98 => "Address already in use\0",
        99 => "Cannot assign requested address\0",
        100 => "Network is down\0",
        101 => "Network is unreachable\0",
        102 => "Network dropped connection on reset\0",
        103 => "Software caused connection abort\0",
        104 => "Connection reset by peer\0",
        105 => "No buffer space available\0",
        106 => "Transport endpoint is already connected\0",
        107 => "Transport endpoint is not connected\0",
        108 => "Cannot send after transport endpoint shutdown\0",
        109 => "Too many references\0",
        110 => "Connection timed out\0",
        111 => "Connection refused\0",
        112 => "Host is down\0",
        113 => "No route to host\0",
        114 => "Operation already in progress\0",
        115 => "Operation now in progress\0",
        116 => "Stale file handle\0",
        117 => "Structure needs cleaning\0",
        118 => "Not a XENIX named type file\0",
        119 => "No XENIX semaphores available\0",
        120 => "Is a named type file\0",
        121 => "Remote I/O error\0",
        122 => "Disk quota exceeded\0",
        123 => "No medium found\0",
        124 => "Wrong medium type\0",
        125 => "Operation canceled\0",
        126 => "Required key not available\0",
        127 => "Key has expired\0",
        128 => "Key has been revoked\0",
        129 => "Key was rejected by service\0",
        130 => "Owner died\0",
        131 => "State not recoverable\0",
        132 => "Operation prevented by radio switch\0",
        133 => "Memory page has hardware error\0",
        _ => return None,
    })
}

/// `strerror`.
pub extern "sysv64" fn strerror(error: c_int) -> *mut c_char {
    // The message is a static string, so handing out a mutable pointer is safe
    // only because callers are contractually forbidden from writing to it.
    errno_message(error).unwrap_or("Unknown error\0").as_ptr() as *mut c_char
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ffs(value: c_int) -> c_int {
    if value == 0 {
        0
    } else {
        value.trailing_zeros() as c_int + 1
    }
}

/// Supported guest locales use the same errno message catalog.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_strerror_l(error: c_int, _locale: *mut c_void) -> *mut c_char {
    strerror(error)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ffsl(value: i64) -> c_int {
    if value == 0 {
        0
    } else {
        value.trailing_zeros() as c_int + 1
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ffsll(value: i64) -> c_int {
    kinakaze_abi_ffsl(value)
}

/// GNU `strerror_r` returns static storage for known errors and the caller's
/// buffer for unknown errors. It does not change errno or allocate memory.
///
/// # Safety
///
/// `buffer` must be writable for `size` bytes.
pub unsafe extern "sysv64" fn strerror_r(
    error: c_int,
    buffer: *mut c_char,
    size: usize,
) -> *mut c_char {
    if let Some(message) = errno_message(error) {
        return message.as_ptr() as *mut c_char;
    }
    let mut digits = [0u8; 11];
    let mut start = digits.len();
    let mut magnitude = error.unsigned_abs();
    loop {
        start -= 1;
        digits[start] = b'0' + (magnitude % 10) as u8;
        magnitude /= 10;
        if magnitude == 0 {
            break;
        }
    }
    if error < 0 {
        start -= 1;
        digits[start] = b'-';
    }
    let mut message = [0u8; 25];
    let prefix = b"Unknown error ";
    message[..prefix.len()].copy_from_slice(prefix);
    let length = prefix.len() + digits.len() - start;
    message[prefix.len()..length].copy_from_slice(&digits[start..]);
    unsafe { copy_error_message(&message[..length], buffer, size) };
    buffer
}

/// Copy without touching errno, including on a truncated or zero-size buffer.
unsafe fn copy_error_message(message: &[u8], buffer: *mut c_char, size: usize) {
    if size == 0 {
        return;
    }
    let copied = message.len().min(size - 1);
    // SAFETY: `copied` is strictly below `size`.
    unsafe {
        ptr::copy_nonoverlapping(message.as_ptr(), buffer.cast::<u8>(), copied);
        *buffer.add(copied) = 0;
    }
}

/// XSI uses a different return ABI: zero on success, a positive error otherwise.
pub unsafe fn xpg_strerror_r(error: c_int, buffer: *mut c_char, size: usize) -> c_int {
    let Some(message) = errno_message(error) else {
        unsafe { strerror_r(error, buffer, size) };
        return kinakaze_vfs::EINVAL;
    };
    let message = &message.as_bytes()[..message.len() - 1];
    unsafe { copy_error_message(message, buffer, size) };
    if size <= message.len() {
        kinakaze_vfs::ERANGE
    } else {
        0
    }
}

/// Unprefixed System V exports for the ELF guest.
#[cfg(test)]
mod description_tests {
    use super::*;
    #[test]
    fn gnu_error_descriptions_are_immutable_complete_and_errno_neutral() {
        let original = kinakaze_tls::errno();
        crate::set_errno(1234);
        for error in 0..=133 {
            let text = exports::kinakaze_abi_strerrordesc_np(error);
            if matches!(error, 41 | 58) {
                assert!(text.is_null());
                continue;
            }
            assert!(!text.is_null(), "errno {error}");
            let value = unsafe { std::ffi::CStr::from_ptr(text) }.to_bytes();
            assert!(!value.is_empty());
            assert_ne!(value, b"Unknown error", "errno {error}");
            assert_eq!(text, exports::kinakaze_abi_strerrordesc_np(error));
        }
        for invalid in [-1, 134, 4096, i32::MAX] {
            assert!(exports::kinakaze_abi_strerrordesc_np(invalid).is_null());
        }
        assert_eq!(kinakaze_tls::errno(), 1234);
        crate::set_errno(original);
    }
}

pub mod exports {
    use super::*;

    /// GNU's non-localized, immutable description. Invalid Linux errno values
    /// return NULL; Nginx uses that distinction while building its error table.
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_strerrordesc_np(error: c_int) -> *const c_char {
        errno_message(error).map_or(ptr::null(), |message| message.as_ptr().cast())
    }

    macro_rules! sysv_export {
        ($alias:ident, $name:ident ( $($argument:ident : $type:ty),* ) -> $result:ty) => {
            #[unsafe(no_mangle)]
            /// System V ABI export. See the wrapped function for the contract.
            ///
            /// # Safety
            ///
            /// Pointer arguments must satisfy the wrapped function's contract.
            pub unsafe extern "sysv64" fn $alias($($argument: $type),*) -> $result {
                // SAFETY: forwarded from this shim's own contract.
                unsafe { super::$name($($argument),*) }
            }
        };
    }

    sysv_export!(kinakaze_abi_strlen, strlen(text: *const c_char) -> usize);
    sysv_export!(kinakaze_abi_strnlen, strnlen(text: *const c_char, limit: usize) -> usize);
    sysv_export!(kinakaze_abi_memcpy, memcpy(destination: *mut c_void, source: *const c_void, count: usize) -> *mut c_void);
    sysv_export!(kinakaze_abi_memmove, memmove(destination: *mut c_void, source: *const c_void, count: usize) -> *mut c_void);
    sysv_export!(kinakaze_abi_memset, memset(destination: *mut c_void, value: c_int, count: usize) -> *mut c_void);
    sysv_export!(kinakaze_abi_memcmp, memcmp(left: *const c_void, right: *const c_void, count: usize) -> c_int);
    sysv_export!(kinakaze_abi_memchr, memchr(haystack: *const c_void, needle: c_int, count: usize) -> *mut c_void);
    sysv_export!(kinakaze_abi_strcpy, strcpy(destination: *mut c_char, source: *const c_char) -> *mut c_char);
    sysv_export!(kinakaze_abi_strncpy, strncpy(destination: *mut c_char, source: *const c_char, limit: usize) -> *mut c_char);
    sysv_export!(kinakaze_abi_strcat, strcat(destination: *mut c_char, source: *const c_char) -> *mut c_char);
    sysv_export!(kinakaze_abi_strncat, strncat(destination: *mut c_char, source: *const c_char, limit: usize) -> *mut c_char);
    sysv_export!(kinakaze_abi_strcmp, strcmp(left: *const c_char, right: *const c_char) -> c_int);
    sysv_export!(kinakaze_abi_strncmp, strncmp(left: *const c_char, right: *const c_char, limit: usize) -> c_int);
    sysv_export!(kinakaze_abi_strchr, strchr(text: *const c_char, needle: c_int) -> *mut c_char);
    sysv_export!(kinakaze_abi_strrchr, strrchr(text: *const c_char, needle: c_int) -> *mut c_char);
    sysv_export!(kinakaze_abi_strstr, strstr(haystack: *const c_char, needle: *const c_char) -> *mut c_char);
    sysv_export!(kinakaze_abi_strspn, strspn(text: *const c_char, accept: *const c_char) -> usize);
    sysv_export!(kinakaze_abi_strcspn, strcspn(text: *const c_char, reject: *const c_char) -> usize);
    sysv_export!(kinakaze_abi_strpbrk, strpbrk(text: *const c_char, accept: *const c_char) -> *mut c_char);
    sysv_export!(kinakaze_abi_strdup, strdup(text: *const c_char) -> *mut c_char);
    sysv_export!(kinakaze_abi_strndup, strndup(text: *const c_char, limit: usize) -> *mut c_char);
    sysv_export!(kinakaze_abi_strerror_r, strerror_r(error: c_int, buffer: *mut c_char, size: usize) -> *mut c_char);

    #[unsafe(no_mangle)]
    /// System V ABI export of the errno message table.
    pub extern "sysv64" fn kinakaze_abi_strerror(error: c_int) -> *mut c_char {
        super::strerror(error)
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn strerror(error: c_int) -> *mut c_char {
        super::strerror(error)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___memset_explicit_chk(
        dest: *mut c_void,
        c: c_int,
        len: usize,
        destlen: usize,
    ) -> *mut c_void {
        if dest.is_null() || len > destlen {
            panic!("__memset_explicit_chk buffer overflow");
        }
        unsafe { ptr::write_bytes(dest as *mut u8, c as u8, len) };
        dest
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __memset_explicit_chk(
        dest: *mut c_void,
        c: c_int,
        len: usize,
        destlen: usize,
    ) -> *mut c_void {
        unsafe { kinakaze_abi___memset_explicit_chk(dest, c, len, destlen) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char {
        unsafe { crate::locale::kinakaze_abi_setlocale(category, locale) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_strlcat(
        dst: *mut c_char,
        src: *const c_char,
        size: usize,
    ) -> usize {
        if dst.is_null() || src.is_null() {
            return 0;
        }
        let dlen = unsafe { super::strlen(dst) };
        let slen = unsafe { super::strlen(src) };
        if size <= dlen {
            return size + slen;
        }
        let to_copy = (size - dlen - 1).min(slen);
        if to_copy > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    src as *const u8,
                    (dst as *mut u8).add(dlen),
                    to_copy,
                );
                *dst.add(dlen + to_copy) = 0;
            }
        } else {
            unsafe {
                *dst.add(dlen) = 0;
            }
        }
        dlen + slen
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn strlcat(
        dst: *mut c_char,
        src: *const c_char,
        size: usize,
    ) -> usize {
        unsafe { kinakaze_abi_strlcat(dst, src, size) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_memmem(
        haystack: *const c_void,
        haystacklen: usize,
        needle: *const c_void,
        needlelen: usize,
    ) -> *mut c_void {
        if needlelen == 0 {
            return haystack as *mut c_void;
        }
        if haystack.is_null() || needle.is_null() || haystacklen < needlelen {
            return core::ptr::null_mut();
        }
        let h = unsafe { core::slice::from_raw_parts(haystack as *const u8, haystacklen) };
        let n = unsafe { core::slice::from_raw_parts(needle as *const u8, needlelen) };
        if let Some(pos) = h.windows(needlelen).position(|w| w == n) {
            unsafe { (haystack as *mut u8).add(pos) as *mut c_void }
        } else {
            core::ptr::null_mut()
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn memmem(
        haystack: *const c_void,
        haystacklen: usize,
        needle: *const c_void,
        needlelen: usize,
    ) -> *mut c_void {
        unsafe { kinakaze_abi_memmem(haystack, haystacklen, needle, needlelen) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___strtok_r(
        str: *mut c_char,
        delim: *const c_char,
        saveptr: *mut *mut c_char,
    ) -> *mut c_char {
        unsafe { crate::misc::kinakaze_abi_strtok_r(str, delim, saveptr) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __strtok_r(
        str: *mut c_char,
        delim: *const c_char,
        saveptr: *mut *mut c_char,
    ) -> *mut c_char {
        unsafe { kinakaze_abi___strtok_r(str, delim, saveptr) }
    }
}
/// Copy through the first matching byte, returning just past that byte.
///
/// # Safety
/// The source/destination must cover `length` bytes and must not overlap.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_memccpy(
    destination: *mut core::ffi::c_void,
    source: *const core::ffi::c_void,
    byte: core::ffi::c_int,
    length: usize,
) -> *mut core::ffi::c_void {
    let source = source.cast::<u8>();
    let destination = destination.cast::<u8>();
    for index in 0..length {
        let value = unsafe { source.add(index).read() };
        unsafe { destination.add(index).write(value) };
        if value == byte as u8 {
            return unsafe { destination.add(index + 1).cast() };
        }
    }
    core::ptr::null_mut()
}
