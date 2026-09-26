//! Environment, pseudo-random numbers, device numbers and string tokenising.
//!
//! Four unrelated corners of `<stdlib.h>`, `<string.h>` and `<sys/sysmacros.h>`
//! that BusyBox reaches for constantly. What they have in common is that each is
//! specified precisely enough that guessing produces a program which runs and
//! then quietly misbehaves: a device number split 8/8 instead of Linux's
//! 12/20-and-8/24, a `rand` sequence that differs from glibc's, or a `putenv`
//! that copies its argument when the caller expects to keep editing it.
//!
//! Environment storage and all mutations live in `process::environment`;
//! the names re-exported here keep the existing unit tests local.

use core::ffi::{CStr, c_char, c_int, c_uint, c_ulonglong, c_void};
use core::ptr;
use std::sync::Mutex;

use kinakaze_vfs::{EFAULT, EINVAL, ENAMETOOLONG};

// ---------------------------------------------------------------------------
// Device numbers.
//
// Linux's `dev_t` is 64 bits and its major/minor split is not the historical
// 8/8 one: the major occupies 12 low bits plus 20 high bits and the minor 8 low
// bits plus 24 high bits, interleaved so that the low 16 bits still look like
// the old 8/8 encoding for small devices. That backwards compatibility is the
// reason for the odd shape, and the reason glibc exposes these as functions —
// a program that open-codes `dev >> 8` works for /dev/sda and breaks for a
// device with a major above 4095.
//
// The masks below are glibc's own from <sys/sysmacros.h>. Writing them as
// explicit 64-bit masks rather than shift-then-mask keeps every bit's
// destination visible, which matters because the two fields interleave.
// ---------------------------------------------------------------------------

/// `gnu_dev_major`: the major number encoded in a Linux `dev_t`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_gnu_dev_major(dev: c_ulonglong) -> c_uint {
    let low = (dev & 0x0000_0000_000f_ff00) >> 8;
    let high = (dev & 0xffff_f000_0000_0000) >> 32;
    (low | high) as c_uint
}

/// `gnu_dev_minor`: the minor number encoded in a Linux `dev_t`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_gnu_dev_minor(dev: c_ulonglong) -> c_uint {
    let low = dev & 0x0000_0000_0000_00ff;
    let high = (dev & 0x0000_0fff_fff0_0000) >> 12;
    (low | high) as c_uint
}

/// `gnu_dev_makedev`: builds a `dev_t` from a major and minor number.
///
/// The four fields tile the 64-bit word exactly, so this round-trips every
/// 32-bit major and minor without loss.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_gnu_dev_makedev(major: c_uint, minor: c_uint) -> c_ulonglong {
    let major = c_ulonglong::from(major);
    let minor = c_ulonglong::from(minor);
    ((major & 0x0000_0fff) << 8)
        | ((major & 0xffff_f000) << 32)
        | (minor & 0x0000_00ff)
        | ((minor & 0xffff_ff00) << 12)
}
#[unsafe(no_mangle)]
// Runtime services can already use native threads before guest pthread_create.
// Conservatively require thread-safe fast paths from the first guest call.
pub static kinakaze_abi___libc_single_threaded: c_char = 0;
// ---------------------------------------------------------------------------
// Pseudo-random numbers.
//
// This is glibc's actual generator, not a stand-in. glibc's default is TYPE_3:
// a 31-entry additive-feedback (lagged Fibonacci) generator, r[i] = r[i-31] +
// r[i-3], with the result shifted right one bit to drop the low-order bit,
// whose period is short. Programs do depend on the exact sequence — test
// suites seed with a constant and compare against recorded output, and BusyBox's
// own testsuite does this — so reproducing the sequence is worth the thirty
// lines it costs.
//
// Seeding fills the 31 entries with the Lehmer generator r[i] = 16807 * r[i-1]
// mod 2147483647, evaluated with Schrage's trick so the intermediate product
// never exceeds 31 bits, then discards 310 outputs to let the feedback mix.
//
// Verified against real glibc: srand(1) yields 1804289383, 846930886,
// 1681692777, 1714636915, 1957747793; srand(42) yields 71876166, 708592740,
// 1483128881.
// ---------------------------------------------------------------------------

/// The largest value `rand` and `random` can return.
const RAND_MAX: i32 = 2_147_483_647;

/// Number of entries in the TYPE_3 state table.
const DEGREE: usize = 31;

/// The feedback lag: r[i-3] is the second term of the recurrence.
const SEPARATION: usize = 3;

/// glibc's additive-feedback generator in rolling form.
///
/// The linear description of the algorithm indexes one long array; keeping 31
/// entries and two cursors is the same recurrence, because the front cursor is
/// always exactly `SEPARATION` ahead of the rear one modulo `DEGREE`.
struct Generator {
    /// The state table. Signed because seeding is specified in signed
    /// arithmetic, where Schrage's trick relies on the sign of the difference.
    state: [i32; DEGREE],
    front: usize,
    rear: usize,
}

impl Generator {
    /// Seeds the generator, exactly as glibc's `srandom` does.
    fn new(seed: c_uint) -> Self {
        // glibc maps seed 0 to 1: a zero state is a fixed point of the Lehmer
        // recurrence and would produce a constant stream.
        let seed = if seed == 0 { 1 } else { seed };
        let mut state = [0i32; DEGREE];
        state[0] = seed as i32;
        for index in 1..DEGREE {
            // Schrage: 16807 * previous mod 2147483647 without overflowing 31
            // bits. `hi` and `lo` split the previous value so that both partial
            // products stay in range.
            let previous = i64::from(state[index - 1]);
            let hi = previous / 127_773;
            let lo = previous % 127_773;
            let mut word = 16_807 * lo - 2_836 * hi;
            if word < 0 {
                word += i64::from(RAND_MAX);
            }
            state[index] = word as i32;
        }
        let mut generator = Self {
            state,
            front: SEPARATION,
            rear: 0,
        };
        // glibc discards 10 * DEGREE outputs so that early values depend on the
        // whole table rather than on the Lehmer sequence alone.
        for _ in 0..(10 * DEGREE) {
            generator.next();
        }
        generator
    }

    /// Produces the next value in `0..=RAND_MAX`.
    fn next(&mut self) -> i32 {
        // Wrapping addition is the specified behaviour: the sum is taken modulo
        // 2^32 and the low bit is then discarded by the shift.
        let sum = (self.state[self.front] as u32).wrapping_add(self.state[self.rear] as u32);
        self.state[self.front] = sum as i32;
        self.front = (self.front + 1) % DEGREE;
        self.rear = (self.rear + 1) % DEGREE;
        (sum >> 1) as i32
    }
}

/// The process-wide generator behind `rand` and `random`.
///
/// `rand` is specified as not thread-safe and as sharing state with `random`,
/// which is why one lock covers both rather than each having a thread-local
/// stream: a guest that seeds on one thread and draws on another must see the
/// seeded sequence. The mutex makes the shared state sound in Rust terms
/// without changing the observable sequence, since glibc locks here too.
static GENERATOR: Mutex<Option<Generator>> = Mutex::new(None);

/// Draws one value, seeding with 1 if the guest never called `srand`.
///
/// An unseeded generator behaving as though seeded with 1 is required by C.
fn draw() -> i32 {
    let Ok(mut generator) = GENERATOR.lock() else {
        // A poisoned lock means a panic crossed a draw. Returning a value from
        // a fresh generator keeps `rand` total, as its signature promises.
        return Generator::new(1).next();
    };
    generator.get_or_insert_with(|| Generator::new(1)).next()
}

/// Reseeds the shared generator.
fn reseed(seed: c_uint) {
    if let Ok(mut generator) = GENERATOR.lock() {
        *generator = Some(Generator::new(seed));
    }
}

/// `rand`: a pseudo-random integer in `0..=RAND_MAX`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_rand() -> c_int {
    draw()
}

/// `srand`: seeds the sequence `rand` returns.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_srand(seed: c_uint) {
    reseed(seed);
}

/// `random`: the POSIX spelling, sharing `rand`'s state as it does in glibc.
///
/// Returns `long`, which is 64 bits in the Linux ABI the guest was compiled for.
/// It is spelled `i64` rather than `c_long` deliberately: `c_long` is 32 bits on
/// Windows, so using it here would leave the high half of `rax` undefined for a
/// guest that reads the full register. The value range is still `0..=RAND_MAX`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_random() -> i64 {
    i64::from(draw())
}

/// `srandom`: seeds the sequence `random` returns.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_srandom(seed: c_uint) {
    reseed(seed);
}

/// `rand_r`: the reentrant generator, whose entire state is `*seed`.
///
/// This is glibc's `rand_r` rather than a reentrant TYPE_3: with only 32 bits of
/// caller state there is nowhere to keep a 31-entry table, so glibc uses three
/// interleaved linear congruential steps and so does this. The sequence
/// therefore differs from `rand`'s, which is true of glibc as well.
///
/// # Safety
///
/// `seed` must point to a writable `unsigned int`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_rand_r(seed: *mut c_uint) -> c_int {
    if seed.is_null() {
        crate::set_errno(EINVAL);
        return -1;
    }
    // The arithmetic below is specified on `unsigned int`, which is 32 bits in
    // both the Linux ABI the guest uses and on this host.
    // SAFETY: the caller guarantees a writable unsigned int.
    let mut next: u32 = unsafe { *seed };

    next = next.wrapping_mul(1_103_515_245).wrapping_add(12_345);
    let mut result = (next / 65_536) % 2_048;

    next = next.wrapping_mul(1_103_515_245).wrapping_add(12_345);
    result <<= 10;
    result ^= (next / 65_536) % 1_024;

    next = next.wrapping_mul(1_103_515_245).wrapping_add(12_345);
    result <<= 10;
    result ^= (next / 65_536) % 1_024;

    // SAFETY: the caller guarantees a writable unsigned int.
    unsafe { *seed = next as c_uint };
    result as c_int
}

// ---------------------------------------------------------------------------
// Numeric conversion.
// ---------------------------------------------------------------------------

/// True for the bytes C's `isspace` accepts in the default locale.
fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `atoll`: leading-whitespace-tolerant `long long` conversion.
///
/// Deliberately has no error reporting. A string with no digits converts to 0
/// and an out-of-range value saturates, both without touching `errno` — that is
/// what distinguishes this from `strtoll`, whose whole purpose is to tell the
/// caller which happened. Saturating rather than wrapping matches glibc, which
/// reaches `atoll` through `strtoll` and keeps its clamped result.
///
/// # Safety
///
/// `text` must be null-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_atoll(text: *const c_char) -> i64 {
    if text.is_null() {
        return 0;
    }
    // SAFETY: the caller guarantees a null-terminated string.
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    let mut at = 0usize;
    while at < bytes.len() && is_space(bytes[at]) {
        at += 1;
    }
    let negative = match bytes.get(at) {
        Some(b'-') => {
            at += 1;
            true
        }
        Some(b'+') => {
            at += 1;
            false
        }
        _ => false,
    };

    // Accumulated as unsigned so that i64::MIN, whose magnitude has no positive
    // counterpart, is representable before the sign is applied.
    let mut magnitude: u64 = 0;
    let limit = if negative {
        i64::MIN.unsigned_abs()
    } else {
        i64::MAX as u64
    };
    let mut saturated = false;
    while let Some(byte) = bytes.get(at) {
        if !byte.is_ascii_digit() {
            break;
        }
        at += 1;
        if saturated {
            continue;
        }
        let digit = u64::from(*byte - b'0');
        match magnitude
            .checked_mul(10)
            .and_then(|scaled| scaled.checked_add(digit))
        {
            Some(next) if next <= limit => magnitude = next,
            _ => {
                magnitude = limit;
                saturated = true;
            }
        }
    }

    if negative {
        // Negating through `wrapping_neg` covers the i64::MIN case, whose
        // magnitude does not fit in an i64 before the sign is applied.
        (magnitude as i64).wrapping_neg()
    } else {
        magnitude as i64
    }
}

// ---------------------------------------------------------------------------
// Tokenising.
// ---------------------------------------------------------------------------

/// `strtok_r`: the reentrant tokeniser.
///
/// All state is the caller's `*saveptr`, which is what makes this usable from
/// two loops at once and inside a library that cannot know whether its caller is
/// mid-`strtok`. The input is modified in place: each returned token is
/// terminated by overwriting its delimiter, so the string must be writable.
///
/// A run of consecutive delimiters yields no empty tokens, which is the
/// difference from `strsep` and the reason a caller wanting empty fields must
/// use that instead.
///
/// # Safety
///
/// `delimiters` must be null-terminated, `saveptr` must be writable, and on the
/// first call `text` must be a writable null-terminated string. On later calls
/// `text` is null and `*saveptr` must be the value the previous call stored.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtok_r(
    text: *mut c_char,
    delimiters: *const c_char,
    saveptr: *mut *mut c_char,
) -> *mut c_char {
    if saveptr.is_null() || delimiters.is_null() {
        return ptr::null_mut();
    }
    // A null `text` continues the previous tokenisation from the saved cursor.
    let mut cursor = if text.is_null() {
        // SAFETY: the caller guarantees `saveptr` is readable.
        unsafe { *saveptr }
    } else {
        text
    };
    if cursor.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees a null-terminated delimiter set.
    let delimiters = unsafe { CStr::from_ptr(delimiters) }.to_bytes();

    // SAFETY: `cursor` addresses a null-terminated string, so every read below
    // stops at or before its terminator.
    unsafe {
        // Skip any leading delimiters. A string that is entirely delimiters ends
        // this loop at the terminator and yields no token.
        while *cursor != 0 && delimiters.contains(&(*cursor as u8)) {
            cursor = cursor.add(1);
        }
        if *cursor == 0 {
            // Park the cursor on the terminator so the next call also reports
            // exhaustion rather than walking off the end.
            *saveptr = cursor;
            return ptr::null_mut();
        }

        let token = cursor;
        while *cursor != 0 && !delimiters.contains(&(*cursor as u8)) {
            cursor = cursor.add(1);
        }
        if *cursor == 0 {
            // Final token: the string's own terminator ends it.
            *saveptr = cursor;
        } else {
            // Terminate the token in place and resume after it.
            *cursor = 0;
            *saveptr = cursor.add(1);
        }
        token
    }
}

pub use crate::process::{kinakaze_abi_clearenv, kinakaze_abi_putenv};

/// `openpty`: allocates a pseudo-terminal and returns both ends.
///
/// Built on the POSIX sequence rather than the other way round, because that is
/// the sequence that has to work anyway: `posix_openpt` creates the terminal,
/// `unlockpt` releases the slave, and `TIOCGPTPEER` opens it. The peer ioctl is
/// used instead of re-opening the name so this cannot be raced by anything that
/// changes what `/dev/pts/N` resolves to between the two steps.
///
/// The previous implementation returned the two ends of a single unidirectional
/// pipe, which meant that writing to the "master" and reading from the "slave"
/// worked, the reverse direction did not, and neither end was a terminal.
///
/// # Safety
///
/// `amaster` and `aslave` must be null or writable `int`s; `name`, when
/// non-null, must have room for the slave's path; `termp` and `winp`, when
/// non-null, must point to a `struct termios` and a `struct winsize`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_openpty(
    amaster: *mut c_int,
    aslave: *mut c_int,
    name: *mut c_char,
    termp: *const c_void,
    winp: *const c_void,
) -> c_int {
    // O_NOCTTY: allocating a terminal must not make it the allocator's own. A
    // shell opening a pty for a child would otherwise lose its own terminal.
    let master = crate::term::kinakaze_abi_posix_openpt(
        kinakaze_vfs::fs::O_RDWR | kinakaze_vfs::fs::O_NOCTTY,
    );
    if master < 0 {
        return -1;
    }
    if crate::term::kinakaze_abi_grantpt(master) < 0
        || crate::term::kinakaze_abi_unlockpt(master) < 0
    {
        crate::kinakaze_abi_close(master);
        return -1;
    }

    // The slave is opened through the master, so it is the peer of *this*
    // terminal by construction rather than by name lookup.
    let slave = unsafe {
        crate::term::kinakaze_abi_ioctl(
            master,
            crate::term::TIOCGPTPEER,
            (kinakaze_vfs::fs::O_RDWR | kinakaze_vfs::fs::O_NOCTTY) as usize as *mut c_void,
        )
    };
    if slave < 0 {
        crate::kinakaze_abi_close(master);
        return -1;
    }

    // The caller's initial settings are applied before anyone can type into the
    // terminal, which is the point of passing them to openpty at all.
    if !termp.is_null() {
        unsafe {
            crate::term::kinakaze_abi_tcsetattr(slave, kinakaze_vfs::pty::TCSANOW, termp.cast())
        };
    }
    if !winp.is_null() {
        unsafe { crate::term::kinakaze_abi_ioctl(slave, crate::term::TIOCSWINSZ, winp.cast_mut()) };
    }

    if !name.is_null() {
        // The buffer's size is not passed by this interface — a design flaw in
        // `openpty` itself — so the name is written through the bounded form
        // with the length every caller's buffer is at least as large as.
        unsafe { crate::term::kinakaze_abi_ptsname_r(master, name, 64) };
    }
    if !amaster.is_null() {
        // SAFETY: the caller guarantees a writable int.
        unsafe { *amaster = master };
    }
    if !aslave.is_null() {
        // SAFETY: the caller guarantees a writable int.
        unsafe { *aslave = slave };
    }
    0
}

/// `forkpty`: forks with the child attached to a fresh pseudo-terminal.
///
/// The child closes the master before `login_tty`, which is not optional: a
/// master left open in the child keeps the terminal alive after the parent
/// closes its own, and the parent's read then never sees end of file.
///
/// # Safety
///
/// As [`kinakaze_abi_openpty`], whose contract the pointers inherit.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_forkpty(
    amaster: *mut c_int,
    name: *mut c_char,
    termp: *const c_void,
    winp: *const c_void,
) -> c_int {
    let mut master = -1;
    let mut slave = -1;
    if unsafe { kinakaze_abi_openpty(&raw mut master, &raw mut slave, name, termp, winp) } < 0 {
        return -1;
    }
    let pid = crate::kinakaze_abi_fork();
    if pid < 0 {
        crate::kinakaze_abi_close(master);
        crate::kinakaze_abi_close(slave);
        return -1;
    }
    if pid == 0 {
        crate::kinakaze_abi_close(master);
        // SAFETY: `slave` is a live descriptor this function opened.
        unsafe { kinakaze_abi_login_tty(slave) };
        return 0;
    }
    // The parent has no use for the slave, and holding it would keep the
    // terminal from ever reporting that the child's last slave closed.
    crate::kinakaze_abi_close(slave);
    if !amaster.is_null() {
        // SAFETY: the caller guarantees a writable int.
        unsafe { *amaster = master };
    }
    pid
}

/// `login_tty`: makes `fd` the calling process's controlling terminal.
///
/// Three steps, and the previous version had only the last of them. `setsid`
/// detaches from whatever session the process was in — a process that is already
/// a session leader is fine, so its failure is not fatal — `TIOCSCTTY` claims
/// the terminal, and only then are the standard descriptors replaced. Without
/// the first two the child has the right file descriptors and the wrong
/// terminal: `/dev/tty` opens somebody else's, and `^C` goes to the wrong group.
///
/// # Safety
///
/// `fd` must be a live descriptor on a terminal.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_login_tty(fd: c_int) -> c_int {
    // A process that is already a session leader gets EPERM here and is already
    // in the state setsid would have produced, so the result is not checked.
    let _ = kinakaze_vfs::job::setsid();
    // SAFETY: TIOCSCTTY takes its argument by value, not by pointer.
    if unsafe { crate::term::kinakaze_abi_ioctl(fd, crate::term::TIOCSCTTY, ptr::null_mut()) } < 0 {
        return -1;
    }
    crate::fdio::kinakaze_abi_dup2(fd, 0);
    crate::fdio::kinakaze_abi_dup2(fd, 1);
    crate::fdio::kinakaze_abi_dup2(fd, 2);
    // The original descriptor is redundant once it has been duplicated onto the
    // standard three, and leaving it open would hold the terminal after those
    // three are closed.
    if fd > 2 {
        crate::kinakaze_abi_close(fd);
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi__dl_find_object(
    address: *const c_void,
    result: *mut c_void,
) -> c_int {
    let Some(services) = kinakaze_runtime::services::unwind() else {
        return -1;
    };
    unsafe { (services.find_object)(address, result.cast()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the tests that touch the shared generator.
    ///
    /// `rand` and `random` are specified to share one process-wide sequence, so
    /// two tests drawing at once would consume each other's values. This is the
    /// honest alternative to pretending the draws are independent.
    fn generator_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A writable NUL-terminated buffer, as the in-place tokeniser requires.
    fn buffer(text: &str) -> Vec<c_char> {
        text.bytes()
            .map(|byte| byte as c_char)
            .chain(std::iter::once(0))
            .collect()
    }

    /// Collects a whole tokenisation into owned strings.
    fn tokenise(text: &mut [c_char], delimiters: &CStr) -> Vec<String> {
        let mut save: *mut c_char = ptr::null_mut();
        let mut first = text.as_mut_ptr();
        let mut tokens = Vec::new();
        loop {
            // SAFETY: the buffer is writable and NUL-terminated, the delimiter
            // set is a valid C string, and `save` is a live local.
            let token = unsafe { kinakaze_abi_strtok_r(first, delimiters.as_ptr(), &raw mut save) };
            if token.is_null() {
                return tokens;
            }
            // SAFETY: a returned token is a NUL-terminated string.
            tokens.push(
                unsafe { CStr::from_ptr(token) }
                    .to_string_lossy()
                    .into_owned(),
            );
            first = ptr::null_mut();
        }
    }

    #[test]
    fn device_numbers_round_trip_beyond_the_historical_8_8_split() {
        // The ordinary case, where the encoding coincides with the old 8/8 one.
        let dev = kinakaze_abi_gnu_dev_makedev(8, 1);
        assert_eq!(kinakaze_abi_gnu_dev_major(dev), 8);
        assert_eq!(kinakaze_abi_gnu_dev_minor(dev), 1);
        // /dev/sda1 really is (8, 1), so the low 16 bits must match the classic
        // encoding for the guest's own arithmetic to agree with ours.
        assert_eq!(dev & 0xffff, 0x0801);

        // A major above 4095 does not fit the low field and must spill into the
        // high one; a naive `dev >> 8` reports the wrong number here.
        let dev = kinakaze_abi_gnu_dev_makedev(70_000, 3);
        assert_eq!(kinakaze_abi_gnu_dev_major(dev), 70_000);
        assert_eq!(kinakaze_abi_gnu_dev_minor(dev), 3);

        // A minor above 255 spills likewise. Device-mapper routinely exceeds it.
        let dev = kinakaze_abi_gnu_dev_makedev(253, 100_000);
        assert_eq!(kinakaze_abi_gnu_dev_major(dev), 253);
        assert_eq!(kinakaze_abi_gnu_dev_minor(dev), 100_000);

        // Both fields at their widest: the four sub-fields tile the word exactly,
        // so even this round-trips.
        let dev = kinakaze_abi_gnu_dev_makedev(u32::MAX, u32::MAX);
        assert_eq!(kinakaze_abi_gnu_dev_major(dev), u32::MAX);
        assert_eq!(kinakaze_abi_gnu_dev_minor(dev), u32::MAX);

        // And zero, which must encode to zero.
        assert_eq!(kinakaze_abi_gnu_dev_makedev(0, 0), 0);
    }

    #[test]
    fn rand_reproduces_the_glibc_sequence() {
        let _serialized = generator_lock();
        // Recorded from real glibc. These are the values a program seeded with 1
        // expects, and the reason this file implements TYPE_3 rather than an LCG.
        kinakaze_abi_srand(1);
        assert_eq!(
            [
                kinakaze_abi_rand(),
                kinakaze_abi_rand(),
                kinakaze_abi_rand(),
                kinakaze_abi_rand(),
                kinakaze_abi_rand(),
            ],
            [1804289383, 846930886, 1681692777, 1714636915, 1957747793]
        );

        kinakaze_abi_srand(42);
        assert_eq!(
            [
                kinakaze_abi_rand(),
                kinakaze_abi_rand(),
                kinakaze_abi_rand()
            ],
            [71876166, 708592740, 1483128881]
        );

        // Seed 0 is mapped to 1, so the two produce the same stream.
        kinakaze_abi_srand(0);
        assert_eq!(kinakaze_abi_rand(), 1804289383);
    }

    #[test]
    fn rand_is_reproducible_and_stays_in_range() {
        let _serialized = generator_lock();
        kinakaze_abi_srand(12345);
        let first: Vec<c_int> = (0..64).map(|_| kinakaze_abi_rand()).collect();
        kinakaze_abi_srand(12345);
        let second: Vec<c_int> = (0..64).map(|_| kinakaze_abi_rand()).collect();
        assert_eq!(first, second, "the same seed must give the same sequence");

        kinakaze_abi_srand(54321);
        let other: Vec<c_int> = (0..64).map(|_| kinakaze_abi_rand()).collect();
        assert_ne!(
            first, other,
            "different seeds must give different sequences"
        );

        // Every draw must be a non-negative int no greater than RAND_MAX.
        kinakaze_abi_srand(7);
        for _ in 0..10_000 {
            let value = kinakaze_abi_rand();
            assert!((0..=RAND_MAX).contains(&value), "out of range: {value}");
        }
    }

    #[test]
    fn random_shares_the_state_rand_uses() {
        let _serialized = generator_lock();
        // glibc's `rand` is a thin wrapper over `random`, so seeding through one
        // spelling must be visible through the other.
        kinakaze_abi_srandom(1);
        assert_eq!(kinakaze_abi_random(), 1804289383);
        assert_eq!(kinakaze_abi_rand(), 846930886);
        kinakaze_abi_srand(1);
        assert_eq!(kinakaze_abi_random(), 1804289383);
    }

    #[test]
    fn rand_r_keeps_its_state_in_the_callers_seed() {
        let mut seed: c_uint = 1;
        // SAFETY: `seed` is a live writable local.
        let first: Vec<c_int> = (0..8)
            .map(|_| unsafe { kinakaze_abi_rand_r(&raw mut seed) })
            .collect();
        let mut seed = 1;
        // SAFETY: as above.
        let again: Vec<c_int> = (0..8)
            .map(|_| unsafe { kinakaze_abi_rand_r(&raw mut seed) })
            .collect();
        assert_eq!(first, again, "the sequence is a function of the seed alone");

        let mut other: c_uint = 2;
        // SAFETY: as above.
        let other: Vec<c_int> = (0..8)
            .map(|_| unsafe { kinakaze_abi_rand_r(&raw mut other) })
            .collect();
        assert_ne!(first, other);
        assert!(first.iter().all(|value| (0..=RAND_MAX).contains(value)));

        // Two callers with private seeds do not disturb each other, which is the
        // whole point of the reentrant form.
        let mut left: c_uint = 99;
        let mut right: c_uint = 99;
        let mut third: c_uint = 99;
        // SAFETY: all three are live writable locals.
        unsafe {
            let a = kinakaze_abi_rand_r(&raw mut left);
            kinakaze_abi_rand_r(&raw mut right);
            let b = kinakaze_abi_rand_r(&raw mut right);
            assert_ne!(a, b, "advancing one seed must change its next draw");
            // A third seed starting where `left` did reproduces `left`'s draw,
            // confirming that `right`'s two calls did not touch shared state.
            assert_eq!(a, kinakaze_abi_rand_r(&raw mut third));
        }

        // A null seed has nowhere to keep state, so it is refused.
        // SAFETY: passing null is the case under test.
        assert_eq!(unsafe { kinakaze_abi_rand_r(ptr::null_mut()) }, -1);
    }

    #[test]
    fn atoll_handles_whitespace_signs_and_values_beyond_32_bits() {
        // SAFETY: every argument below is a literal C string.
        unsafe {
            assert_eq!(kinakaze_abi_atoll(c"0".as_ptr()), 0);
            assert_eq!(kinakaze_abi_atoll(c"42".as_ptr()), 42);
            // Leading whitespace of every accepted kind is skipped.
            assert_eq!(kinakaze_abi_atoll(c"   \t\n\r\x0b\x0c 17".as_ptr()), 17);
            assert_eq!(kinakaze_abi_atoll(c"+17".as_ptr()), 17);
            assert_eq!(kinakaze_abi_atoll(c"  -17".as_ptr()), -17);

            // Past 32 bits, which is the whole reason `atoll` exists next to `atoi`.
            assert_eq!(
                kinakaze_abi_atoll(c"9007199254740993".as_ptr()),
                9_007_199_254_740_993
            );
            assert_eq!(
                kinakaze_abi_atoll(c"-9007199254740993".as_ptr()),
                -9_007_199_254_740_993
            );

            // The extremes are exactly representable, including the asymmetric
            // negative one.
            assert_eq!(
                kinakaze_abi_atoll(c"9223372036854775807".as_ptr()),
                i64::MAX
            );
            assert_eq!(
                kinakaze_abi_atoll(c"-9223372036854775808".as_ptr()),
                i64::MIN
            );

            // Conversion stops at the first non-digit and reports no error.
            assert_eq!(kinakaze_abi_atoll(c"12abc".as_ptr()), 12);
            assert_eq!(kinakaze_abi_atoll(c"abc".as_ptr()), 0);
            assert_eq!(kinakaze_abi_atoll(c"".as_ptr()), 0);
            assert_eq!(kinakaze_abi_atoll(c"-".as_ptr()), 0);

            // Out of range saturates rather than wrapping, and still sets no errno.
            assert_eq!(
                kinakaze_abi_atoll(c"99999999999999999999999".as_ptr()),
                i64::MAX
            );
            assert_eq!(
                kinakaze_abi_atoll(c"-99999999999999999999999".as_ptr()),
                i64::MIN
            );
        }
    }

    #[test]
    fn strtok_r_handles_delimiter_runs_at_every_position() {
        // Leading, consecutive and trailing delimiters all collapse: the
        // tokeniser never returns an empty token.
        let mut text = buffer("::a::bb:::ccc::");
        assert_eq!(tokenise(&mut text, c":"), ["a", "bb", "ccc"]);

        // A single token with no delimiters at all.
        let mut text = buffer("alone");
        assert_eq!(tokenise(&mut text, c":"), ["alone"]);

        // A delimiter set that matches nothing leaves the string whole.
        let mut text = buffer("a:b:c");
        assert_eq!(tokenise(&mut text, c",;"), ["a:b:c"]);

        // A string of nothing but delimiters yields nothing.
        let mut text = buffer(":::");
        assert!(tokenise(&mut text, c":").is_empty());

        // And an empty string yields nothing.
        let mut text = buffer("");
        assert!(tokenise(&mut text, c":").is_empty());

        // Multiple delimiter characters are each accepted.
        let mut text = buffer("one, two;three  four");
        assert_eq!(tokenise(&mut text, c" ,;"), ["one", "two", "three", "four"]);

        // Exhaustion is sticky: further calls keep reporting the end.
        let mut text = buffer("x");
        let mut save: *mut c_char = ptr::null_mut();
        // SAFETY: the buffer is writable and NUL-terminated.
        unsafe {
            let first = kinakaze_abi_strtok_r(text.as_mut_ptr(), c":".as_ptr(), &raw mut save);
            assert!(!first.is_null());
            assert!(kinakaze_abi_strtok_r(ptr::null_mut(), c":".as_ptr(), &raw mut save).is_null());
            assert!(kinakaze_abi_strtok_r(ptr::null_mut(), c":".as_ptr(), &raw mut save).is_null());
        }
    }

    #[test]
    fn two_tokenisations_share_no_state() {
        // The reason `strtok_r` exists: interleaving two scans must not disturb
        // either, which the static-state `strtok` cannot manage.
        let mut left = buffer("a:b:c");
        let mut right = buffer("x|y|z");
        let mut left_save: *mut c_char = ptr::null_mut();
        let mut right_save: *mut c_char = ptr::null_mut();

        // SAFETY: both buffers are writable and NUL-terminated and both save
        // slots are live locals.
        unsafe {
            let read = |token: *mut c_char| CStr::from_ptr(token).to_string_lossy().into_owned();

            let l1 = kinakaze_abi_strtok_r(left.as_mut_ptr(), c":".as_ptr(), &raw mut left_save);
            let r1 = kinakaze_abi_strtok_r(right.as_mut_ptr(), c"|".as_ptr(), &raw mut right_save);
            assert_eq!(read(l1), "a");
            assert_eq!(read(r1), "x");

            let r2 = kinakaze_abi_strtok_r(ptr::null_mut(), c"|".as_ptr(), &raw mut right_save);
            let l2 = kinakaze_abi_strtok_r(ptr::null_mut(), c":".as_ptr(), &raw mut left_save);
            assert_eq!(read(r2), "y");
            assert_eq!(read(l2), "b");

            let l3 = kinakaze_abi_strtok_r(ptr::null_mut(), c":".as_ptr(), &raw mut left_save);
            let r3 = kinakaze_abi_strtok_r(ptr::null_mut(), c"|".as_ptr(), &raw mut right_save);
            assert_eq!(read(l3), "c");
            assert_eq!(read(r3), "z");

            assert!(
                kinakaze_abi_strtok_r(ptr::null_mut(), c":".as_ptr(), &raw mut left_save).is_null()
            );
            assert!(
                kinakaze_abi_strtok_r(ptr::null_mut(), c"|".as_ptr(), &raw mut right_save)
                    .is_null()
            );
        }
    }

    #[test]
    fn putenv_installs_the_callers_string_without_copying_it() {
        // Leaked deliberately: `putenv` does not copy, so the string must outlive
        // the binding. A stack buffer here would be the use-after-free the doc
        // comment warns about.
        let entry: &'static mut [c_char] =
            Box::leak(buffer("KINAKAZE_PUTENV_NOCOPY=one").into_boxed_slice());
        // SAFETY: the entry is a live NUL-terminated string that is never freed.
        assert_eq!(unsafe { kinakaze_abi_putenv(entry.as_mut_ptr()) }, 0);

        let name = c"KINAKAZE_PUTENV_NOCOPY";
        // SAFETY: `name` is a literal C string.
        let found = unsafe { crate::process::kinakaze_abi_getenv(name.as_ptr()) };
        assert!(!found.is_null(), "putenv did not create the binding");
        // SAFETY: `getenv` returns a pointer into a NUL-terminated entry.
        assert_eq!(unsafe { CStr::from_ptr(found) }.to_bytes(), b"one");

        // The returned pointer must address the caller's own string, not a copy.
        // That is the specified difference from `setenv` and what lets the caller
        // edit in place.
        let value_offset = "KINAKAZE_PUTENV_NOCOPY=".len();
        assert_eq!(
            found as usize,
            entry.as_ptr() as usize + value_offset,
            "putenv copied the string instead of adopting it"
        );

        // Edit through the caller's buffer; the environment must follow. The
        // replacement is the same length, since growing it would need a
        // reallocation the environment knows nothing about.
        for (slot, byte) in entry[value_offset..value_offset + 3].iter_mut().zip(b"two") {
            *slot = *byte as c_char;
        }
        // SAFETY: `name` is a literal C string.
        let found = unsafe { crate::process::kinakaze_abi_getenv(name.as_ptr()) };
        // SAFETY: `getenv` returns a pointer into a NUL-terminated entry.
        assert_eq!(
            unsafe { CStr::from_ptr(found) }.to_bytes(),
            b"two",
            "an in-place edit was not visible through getenv"
        );

        // `environ` itself must show the same entry, so a guest walking the block
        // by hand agrees with `getenv`.
        let block = crate::process::kinakaze_abi_environ.get();
        assert!(!block.is_null());
        let mut seen = false;
        let mut cursor = 0usize;
        loop {
            // SAFETY: `environ` is a NUL-terminated array of C strings.
            let slot = unsafe { *block.add(cursor) };
            if slot.is_null() {
                break;
            }
            if std::ptr::eq(slot, entry.as_ptr()) {
                seen = true;
            }
            cursor += 1;
        }
        assert!(seen, "environ does not name the string putenv was given");

        // A string with no '=' removes the variable, as glibc's putenv does.
        let mut removal = buffer("KINAKAZE_PUTENV_NOCOPY");
        // SAFETY: a live NUL-terminated string; the removal path does not retain it.
        assert_eq!(unsafe { kinakaze_abi_putenv(removal.as_mut_ptr()) }, 0);
        // SAFETY: `name` is a literal C string.
        assert!(unsafe { crate::process::kinakaze_abi_getenv(name.as_ptr()) }.is_null());
    }

    #[test]
    fn putenv_rejects_an_entry_with_no_name() {
        let mut entry = buffer("=novalue");
        // SAFETY: a live NUL-terminated string.
        assert_eq!(unsafe { kinakaze_abi_putenv(entry.as_mut_ptr()) }, -1);
        // SAFETY: passing null is the case under test.
        assert_eq!(unsafe { kinakaze_abi_putenv(ptr::null_mut()) }, -1);
    }

    /// Drives the destructive `clearenv` check in a child process.
    ///
    /// `clearenv` empties the environment of the whole process, and the test
    /// harness runs tests on threads that share it: doing this in-process would
    /// delete variables another test had just set. Re-running this binary for the
    /// single ignored test below is what keeps the check honest without making
    /// its neighbours flaky.
    #[test]
    fn clearenv_leaves_a_valid_empty_environment() {
        let Ok(executable) = std::env::current_exe() else {
            panic!("the test binary must be locatable to re-run itself");
        };
        let output = std::process::Command::new(executable)
            .args([
                "misc::tests::clearenv_empties_the_environment_in_place",
                "--exact",
                "--include-ignored",
                "--test-threads=1",
            ])
            .output()
            .expect("failed to re-run the test binary");
        assert!(
            output.status.success(),
            "clearenv child failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    #[ignore = "empties this process's environment; driven by clearenv_leaves_a_valid_empty_environment"]
    fn clearenv_empties_the_environment_in_place() {
        let name = c"KINAKAZE_CLEARENV_VICTIM";
        // SAFETY: both are literal C strings.
        unsafe {
            assert_eq!(
                crate::process::kinakaze_abi_setenv(name.as_ptr(), c"present".as_ptr(), 1),
                0
            );
            assert!(!crate::process::kinakaze_abi_getenv(name.as_ptr()).is_null());
        }

        assert_eq!(kinakaze_abi_clearenv(), 0);

        // Match glibc: clearenv publishes a null environment pointer.
        assert!(crate::process::kinakaze_abi_environ.get().is_null());

        // Nothing is findable afterwards.
        // SAFETY: `name` is a literal C string.
        assert!(unsafe { crate::process::kinakaze_abi_getenv(name.as_ptr()).is_null() });
        // Guest mutations do not add a Windows environment binding.
        assert!(std::env::var_os("KINAKAZE_CLEARENV_VICTIM").is_none());

        // The environment must still be usable: a fresh binding after clearing
        // has to appear in the same block.
        // SAFETY: both are literal C strings.
        unsafe {
            assert_eq!(
                crate::process::kinakaze_abi_setenv(name.as_ptr(), c"again".as_ptr(), 1),
                0
            );
            let found = crate::process::kinakaze_abi_getenv(name.as_ptr());
            assert!(
                !found.is_null(),
                "the environment was unusable after clearing"
            );
            assert_eq!(CStr::from_ptr(found).to_bytes(), b"again");
        }
    }
}

// ---------------------------------------------------------------------------
// Locale-passthrough stubs.
// These are the `_l` variants that accept a `locale_t` argument.  The locale
// argument is silently ignored; all output follows the POSIX/C locale, which is
// the only locale we support.
// ---------------------------------------------------------------------------

/// Opaque locale handle type (pointer-sized).
type LocaleT = *mut c_void;

/// Integer conversion in the supported C/C.UTF-8 locale categories.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtoll_l(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
    _locale: LocaleT,
) -> i64 {
    unsafe { crate::process::kinakaze_abi_strtoll(text, end, base) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtoull_l(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
    _locale: LocaleT,
) -> u64 {
    unsafe { crate::process::kinakaze_abi_strtoull(text, end, base) }
}

/// `__strtod_l`.
///
/// # Safety
/// `text` must be null-terminated, `end` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___strtod_l(
    text: *const c_char,
    end: *mut *const c_char,
    _locale: LocaleT,
) -> f64 {
    unsafe { crate::process::kinakaze_abi_strtod(text, end) }
}

/// `__strtof_l`.
///
/// # Safety
/// `text` must be null-terminated, `end` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___strtof_l(
    text: *const c_char,
    end: *mut *const c_char,
    _locale: LocaleT,
) -> f32 {
    unsafe { crate::process::kinakaze_abi_strtof(text, end) }
}

/// `__strftime_l` — same as `strftime`, locale ignored.
///
/// # Safety
/// See `strftime` contract.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___strftime_l(
    s: *mut c_char,
    max: usize,
    format: *const c_char,
    tm: *const c_void,
    _locale: LocaleT,
) -> usize {
    unsafe { crate::time::kinakaze_abi_strftime(s, max, format, tm.cast::<crate::time::Tm>()) }
}

/// `pathconf` — returns -1 (unknown) for all path limits.
///
/// # Safety
/// `path` must be null-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pathconf(_path: *const c_char, name: c_int) -> i64 {
    // Return known safe defaults for common names.
    match name {
        0 => 255,  // _PC_LINK_MAX
        1 => 4096, // _PC_MAX_CANON
        4 => 255,  // _PC_NAME_MAX
        5 => 4096, // _PC_PATH_MAX
        _ => -1,
    }
}

/// `fpathconf` — same as pathconf but for a file descriptor.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fpathconf(_fd: c_int, name: c_int) -> i64 {
    match name {
        0 => 255,
        1 => 4096,
        4 => 255,
        5 => 4096,
        _ => -1,
    }
}

/// `gai_strerror` — return a string for getaddrinfo error codes.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_gai_strerror(errcode: c_int) -> *const c_char {
    // Return a static string for the most common codes.
    match errcode {
        0 => c"Success".as_ptr(),
        -2 => c"Name or service not known".as_ptr(),
        -3 => c"Temporary failure in name resolution".as_ptr(),
        _ => c"Unknown error".as_ptr(),
    }
}

/// `if_indextoname` — stub, always returns null.
///
/// # Safety
/// `name` must be writable for `IF_NAMESIZE` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_if_indextoname(
    _ifindex: c_uint,
    _name: *mut c_char,
) -> *mut c_char {
    core::ptr::null_mut()
}

/// `sendfile` — copies data between file descriptors.
///
/// # Safety
/// `offset` must be null or point to a writable `off_t`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sendfile(
    out_fd: c_int,
    in_fd: c_int,
    offset: *mut i64,
    count: usize,
) -> isize {
    if !offset.is_null() {
        // Seek to the given offset.
        let off = unsafe { *offset };
        match kinakaze_vfs::fs::lseek(in_fd, off, kinakaze_vfs::fs::SEEK_SET) {
            Ok(_) => {}
            Err(e) => {
                crate::set_errno(e);
                return -1;
            }
        }
    }
    // Copy up to count bytes from in_fd to out_fd.
    let mut buf = [0u8; 65536];
    let to_read = count.min(buf.len());
    let nread = match kinakaze_vfs::read(in_fd, &mut buf[..to_read]) {
        Ok(n) => n,
        Err(e) => {
            crate::set_errno(e);
            return -1;
        }
    };
    if !offset.is_null() {
        unsafe {
            *offset += nread as i64;
        }
    }
    if nread == 0 {
        return 0;
    }
    match kinakaze_vfs::write(out_fd, &buf[..nread]) {
        Ok(n) => n as isize,
        Err(e) => {
            crate::set_errno(e);
            -1
        }
    }
}

#[repr(C)]
struct MultiMessage {
    header: crate::netdb::MsgHdr,
    length: u32,
}

/// Batch send; each message uses the same ancillary validation and ownership
/// transaction as sendmsg. A later error returns the completed message count.
///
/// # Safety
/// `msgvec` must be valid for `vlen` entries.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sendmmsg(
    sockfd: c_int,
    msgvec: *mut c_void,
    vlen: c_uint,
    flags: c_int,
) -> c_int {
    if vlen == 0 {
        return 0;
    }
    if msgvec.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    for index in 0..vlen.min(1024) {
        let message = unsafe { &mut *msgvec.cast::<MultiMessage>().add(index as usize) };
        let sent = unsafe { crate::netdb::kinakaze_abi_sendmsg(sockfd, &message.header, flags) };
        if sent < 0 {
            return if index == 0 { -1 } else { index as c_int };
        }
        message.length = sent as u32;
    }
    vlen.min(1024) as c_int
}

/// Batch receive with a relative timeout and MSG_WAITFORONE. Received rights
/// belong to their individual message, including partial batch completion.
///
/// # Safety
/// `msgvec` must be valid for `vlen` entries.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_recvmmsg(
    sockfd: c_int,
    msgvec: *mut c_void,
    vlen: c_uint,
    flags: c_int,
    timeout: *mut c_void,
) -> c_int {
    if vlen == 0 {
        return 0;
    }
    if msgvec.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let started = std::time::Instant::now();
    let duration = if timeout.is_null() {
        None
    } else {
        let value = unsafe { timeout.cast::<crate::fdio::Timespec>().read_unaligned() };
        if value.tv_sec < 0 || !(0..1_000_000_000).contains(&value.tv_nsec) {
            crate::set_errno(kinakaze_vfs::EINVAL);
            return -1;
        }
        Some(std::time::Duration::new(
            value.tv_sec as u64,
            value.tv_nsec as u32,
        ))
    };
    let mut count = 0;
    let mut result = 0;
    'messages: while count < vlen.min(1024) {
        let message = unsafe { &mut *msgvec.cast::<MultiMessage>().add(count as usize) };
        let dontwait = flags & 0x40 != 0
            || (count != 0 && flags & 0x10000 != 0)
            || kinakaze_vfs::get(sockfd)
                .is_ok_and(|e| e.flags.contains(kinakaze_vfs::FdFlags::NONBLOCK));
        let recv_flags = (flags & !0x10000) | if duration.is_some() { 0x40 } else { 0 };
        loop {
            let received = unsafe {
                crate::netdb::kinakaze_abi_recvmsg(
                    sockfd,
                    &mut message.header,
                    recv_flags | if dontwait { 0x40 } else { 0 },
                )
            };
            if received >= 0 {
                message.length = received as u32;
                count += 1;
                result = count as c_int;
                continue 'messages;
            }
            let error = unsafe { *crate::kinakaze___errno_location() };
            if error != kinakaze_vfs::EAGAIN || dontwait || duration.is_none() {
                result = if count == 0 { -1 } else { count as c_int };
                break 'messages;
            }
            let remaining = duration.unwrap().saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break 'messages;
            }
            let mut poll = crate::fdio::PollFd {
                fd: sockfd,
                events: 1,
                revents: 0,
            };
            let wait = remaining
                .as_millis()
                .saturating_add(u128::from(remaining.subsec_nanos() % 1_000_000 != 0))
                .min(i32::MAX as u128) as i32;
            if unsafe { crate::fdio::kinakaze_abi_poll(&mut poll, 1, wait) } < 0 {
                result = if count == 0 { -1 } else { count as c_int };
                break 'messages;
            }
        }
    }
    if let Some(duration) = duration {
        let remaining = duration.saturating_sub(started.elapsed());
        unsafe {
            timeout
                .cast::<crate::fdio::Timespec>()
                .write_unaligned(crate::fdio::Timespec {
                    tv_sec: remaining.as_secs() as i64,
                    tv_nsec: remaining.subsec_nanos() as i64,
                });
        }
    }
    result
}

/// `makecontext` — stub; ucontext not supported.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_makecontext(
    _ucp: *mut c_void,
    _func: *const c_void,
    _argc: c_int,
) {
    // No-op.
}

/// `getcontext` — stub; returns -1.
///
/// # Safety
/// `ucp` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getcontext(_ucp: *mut c_void) -> c_int {
    crate::set_errno(kinakaze_vfs::ENOSYS);
    -1
}

/// `setcontext` — stub; returns -1.
///
/// # Safety
/// `ucp` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setcontext(_ucp: *const c_void) -> c_int {
    crate::set_errno(kinakaze_vfs::ENOSYS);
    -1
}

/// `msync` — flush memory-mapped writes to file.
///
/// # Safety
/// `addr` must be a valid mapped address.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_msync(
    addr: *mut c_void,
    length: usize,
    flags: c_int,
) -> c_int {
    match crate::fdio::sync_mappings(addr as usize, length, flags) {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

fn inotify_result(result: Result<c_int, i32>) -> c_int {
    match result {
        Ok(value) => value,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// Borrows an inotify guest path without changing its byte representation.
///
/// # Safety
/// `pathname` must be null or point to a NUL-terminated guest string.
unsafe fn borrow_inotify_path(pathname: *const c_char) -> Result<&'static str, i32> {
    if pathname.is_null() {
        return Err(EFAULT);
    }
    // SAFETY: guaranteed by the caller's C ABI contract.
    let path = unsafe { CStr::from_ptr(pathname) };
    if path.to_bytes().len() > 4096 {
        return Err(ENAMETOOLONG);
    }
    path.to_str().map_err(|_| EINVAL)
}

/// `inotify_init`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_inotify_init() -> c_int {
    kinakaze_abi_inotify_init1(0)
}

/// `inotify_init1`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_inotify_init1(flags: c_int) -> c_int {
    inotify_result(kinakaze_vfs::inotify::create_inotify(flags))
}

/// `inotify_add_watch`.
///
/// # Safety
/// `pathname` must be null-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_inotify_add_watch(
    fd: c_int,
    pathname: *const c_char,
    mask: u32,
) -> c_int {
    // SAFETY: forwarded from this function's C ABI contract.
    let path = unsafe { borrow_inotify_path(pathname) };
    inotify_result(path.and_then(|path| kinakaze_vfs::inotify::add_watch_linux(fd, path, mask)))
}

/// `inotify_rm_watch`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_inotify_rm_watch(fd: c_int, wd: c_int) -> c_int {
    match kinakaze_vfs::inotify::rm_watch(fd, wd) {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `__wcsftime_l` — wide char strftime (stub, returns 0).
///
/// # Safety
/// Pointers must satisfy wcsftime contract.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___wcsftime_l(
    _s: *mut i32,
    _max: usize,
    _format: *const i32,
    _tm: *const c_void,
    _locale: LocaleT,
) -> usize {
    0
}

/// `mbsnrtowcs` — stub, always returns 0.
///
/// # Safety
/// Pointers must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mbsnrtowcs(
    _dst: *mut i32,
    _src: *mut *const c_char,
    _nmc: usize,
    _len: usize,
    _ps: *mut c_void,
) -> usize {
    0
}

/// `wcsnrtombs` — stub, always returns 0.
///
/// # Safety
/// Pointers must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wcsnrtombs(
    _dst: *mut c_char,
    _src: *mut *const i32,
    _nwc: usize,
    _len: usize,
    _ps: *mut c_void,
) -> usize {
    0
}

/// `mremap` — stub, always fails with ENOSYS.
///
/// # Safety
/// `_old_address` must be a valid mmap region.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mremap(
    _old_address: *mut c_void,
    _old_size: usize,
    _new_size: usize,
    _flags: c_int,
) -> *mut c_void {
    crate::set_errno(kinakaze_vfs::ENOSYS);
    // MAP_FAILED = (void *)-1
    usize::MAX as *mut c_void
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ctermid(s: *mut c_char) -> *mut c_char {
    static CTERMID_BUF: [c_char; 10] = [
        b'/' as c_char,
        b'd' as c_char,
        b'e' as c_char,
        b'v' as c_char,
        b'/' as c_char,
        b't' as c_char,
        b't' as c_char,
        b'y' as c_char,
        0,
        0,
    ];
    let dest = if s.is_null() {
        CTERMID_BUF.as_ptr() as *mut c_char
    } else {
        s
    };
    if !s.is_null() {
        unsafe { core::ptr::copy_nonoverlapping(CTERMID_BUF.as_ptr(), s, 9) };
    }
    dest
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getloadavg(loadavg: *mut f64, nelem: c_int) -> c_int {
    if loadavg.is_null() || nelem <= 0 {
        return -1;
    }
    let count = (nelem as usize).min(3);
    for i in 0..count {
        unsafe { *loadavg.add(i) = 0.5 };
    }
    count as c_int
}

static PROGNAME: [c_char; 8] = [
    b'p' as c_char,
    b'r' as c_char,
    b'o' as c_char,
    b'g' as c_char,
    b'r' as c_char,
    b'a' as c_char,
    b'm' as c_char,
    0,
];

#[unsafe(no_mangle)]
pub static kinakaze_abi_program_invocation_name: crate::copied::CopiedPointer =
    crate::copied::CopiedPointer::with_value(PROGNAME.as_ptr().cast_mut());

#[unsafe(no_mangle)]
pub static program_invocation_name: crate::copied::CopiedPointer =
    crate::copied::CopiedPointer::with_value(PROGNAME.as_ptr().cast_mut());

#[unsafe(no_mangle)]
pub static kinakaze_abi_program_invocation_short_name: crate::copied::CopiedPointer =
    crate::copied::CopiedPointer::with_value(PROGNAME.as_ptr().cast_mut());

#[unsafe(no_mangle)]
pub static program_invocation_short_name: crate::copied::CopiedPointer =
    crate::copied::CopiedPointer::with_value(PROGNAME.as_ptr().cast_mut());

#[unsafe(no_mangle)]
pub static kinakaze_abi___progname: crate::copied::CopiedPointer =
    crate::copied::CopiedPointer::with_value(PROGNAME.as_ptr().cast_mut());

#[unsafe(no_mangle)]
pub static __progname: crate::copied::CopiedPointer =
    crate::copied::CopiedPointer::with_value(PROGNAME.as_ptr().cast_mut());

#[unsafe(no_mangle)]
pub static kinakaze_abi___progname_full: crate::copied::CopiedPointer =
    crate::copied::CopiedPointer::with_value(PROGNAME.as_ptr().cast_mut());

#[unsafe(no_mangle)]
pub static __progname_full: crate::copied::CopiedPointer =
    crate::copied::CopiedPointer::with_value(PROGNAME.as_ptr().cast_mut());

/// Publishes glibc's program-name globals from the guest's real `argv[0]`.
///
/// The pointers keep referring into the initial guest stack, whose lifetime is
/// the process. COPY-relocated executables are updated through `CopiedPointer`
/// redirects, so OpenSSH and other programs never see the hosting loader name.
pub(crate) fn set_program_invocation(argv0: *mut c_char) {
    if argv0.is_null() {
        return;
    }
    // SAFETY: argv[0] is a NUL-terminated string on the initial guest stack.
    let bytes = unsafe { CStr::from_ptr(argv0) }.to_bytes();
    let short_offset = bytes
        .iter()
        .rposition(|byte| matches!(*byte, b'/' | b'\\'))
        .map_or(0, |index| index + 1);
    // SAFETY: the offset is within the same live argv[0] allocation.
    let short = unsafe { argv0.add(short_offset) };

    for variable in [
        &kinakaze_abi_program_invocation_name,
        &program_invocation_name,
        &kinakaze_abi___progname_full,
        &__progname_full,
    ] {
        variable.set(argv0);
    }
    for variable in [
        &kinakaze_abi_program_invocation_short_name,
        &program_invocation_short_name,
        &kinakaze_abi___progname,
        &__progname,
    ] {
        variable.set(short);
    }
    if std::env::var_os("KINAKAZE_DEBUG_STARTUP").is_some() {
        eprintln!(
            "kinakaze: [STARTUP] program name full={:?} short={:?} exported={:?}",
            argv0,
            short,
            __progname.get()
        );
    }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___gmon_start__() {}

#[unsafe(no_mangle)]
pub extern "sysv64" fn __gmon_start__() {}

#[unsafe(no_mangle)]
pub extern "sysv64" fn _ITM_deregisterTMCloneTable() {}

#[unsafe(no_mangle)]
pub extern "sysv64" fn _ITM_registerTMCloneTable() {}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___tls_get_addr(ti: *const usize) -> *mut c_void {
    if ti.is_null() {
        return core::ptr::null_mut();
    }
    let module = unsafe { *ti };
    let offset = unsafe { *ti.add(1) };
    kinakaze_tls::elf_tls_get_addr(module, offset)
        .unwrap_or(core::ptr::null_mut())
        .cast()
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __tls_get_addr(ti: *const usize) -> *mut c_void {
    unsafe { kinakaze_abi___tls_get_addr(ti) }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___popcountdi2(a: u64) -> c_int {
    a.count_ones() as c_int
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___udivti3(a: u128, b: u128) -> u128 {
    if b == 0 { 0 } else { a / b }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___udivmodti4(a: u128, b: u128, rem: *mut u128) -> u128 {
    if b == 0 {
        return 0;
    }
    if !rem.is_null() {
        unsafe {
            *rem = a % b;
        }
    }
    a / b
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___eqtf2(_a: u128, _b: u128) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___getf2(_a: u128, _b: u128) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___letf2(_a: u128, _b: u128) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___unordtf2(_a: u128, _b: u128) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___fpclassify(x: f64) -> c_int {
    if x.is_nan() {
        0
    } else if x.is_infinite() {
        1
    } else if x == 0.0 {
        2
    } else if x.is_subnormal() {
        3
    } else {
        4
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_swapcontext(
    _oucp: *mut c_void,
    _ucp: *const c_void,
) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___cxa_pure_virtual() -> ! {
    eprintln!("pure virtual method called");
    std::process::abort()
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___cxa_guard_acquire(guard: *mut u64) -> c_int {
    if guard.is_null() {
        return 1;
    }
    let first_byte = guard as *mut u8;
    if unsafe { *first_byte == 0 } {
        unsafe { *first_byte = 1 };
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___cxa_guard_release(_guard: *mut u64) {}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___cxa_thread_atexit(
    dtor: Option<unsafe extern "sysv64" fn(*mut c_void)>,
    obj: *mut c_void,
    dso_symbol: *mut c_void,
) -> c_int {
    match kinakaze_tls::cxa_thread_atexit(dtor, obj, dso_symbol) {
        Ok(()) => 0,
        Err(error) => {
            kinakaze_tls::set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___cxa_demangle(
    mangled_name: *const c_char,
    output_buffer: *mut c_char,
    length: *mut usize,
    status: *mut c_int,
) -> *mut c_char {
    if !status.is_null() {
        unsafe {
            *status = 0;
        }
    }
    if mangled_name.is_null() {
        return core::ptr::null_mut();
    }
    let len = unsafe { core::ffi::CStr::from_ptr(mangled_name) }
        .to_bytes()
        .len()
        + 1;
    let buf = if output_buffer.is_null() {
        unsafe { kinakaze_alloc::c::malloc(len) as *mut c_char }
    } else {
        output_buffer
    };
    if !buf.is_null() {
        unsafe {
            core::ptr::copy_nonoverlapping(mangled_name, buf, len);
        }
        if !length.is_null() {
            unsafe {
                *length = len;
            }
        }
    }
    buf
}

#[link(name = "advapi32")]
unsafe extern "system" {
    #[link_name = "SystemFunction036"]
    fn RtlGenRandom(RandomBuffer: *mut c_void, RandomBufferLength: u32) -> u8;
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_arc4random_buf(buffer: *mut c_void, length: usize) {
    if buffer.is_null() || length == 0 {
        return;
    }
    let mut remaining = length;
    let mut ptr = buffer as *mut u8;
    while remaining > 0 {
        let chunk = remaining.min(u32::MAX as usize) as u32;
        let ok = unsafe { RtlGenRandom(ptr as *mut c_void, chunk) };
        if ok == 0 {
            break;
        }
        remaining -= chunk as usize;
        ptr = unsafe { ptr.add(chunk as usize) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_arc4random_uniform(upper_bound: u32) -> u32 {
    if upper_bound < 2 {
        return 0;
    }
    let min = (0u32.wrapping_sub(upper_bound)) % upper_bound;
    loop {
        let r = unsafe { crate::sysadmin::kinakaze_abi_arc4random() };
        if r >= min {
            return r % upper_bound;
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___fsetlocking(
    _stream: *mut c_void,
    _type: c_int,
) -> c_int {
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_reallocarray(
    ptr: *mut c_void,
    nmemb: usize,
    size: usize,
) -> *mut c_void {
    let total = match nmemb.checked_mul(size) {
        Some(t) => t,
        None => {
            crate::set_errno(kinakaze_vfs::ENOMEM);
            return core::ptr::null_mut();
        }
    };
    unsafe { crate::c_realloc(ptr, total) }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_malloc_trim(_pad: usize) -> c_int {
    1
}
