//! Clocks, timers, broken-down time and time formatting.
//!
//! Everything here answers one of three questions: what time is it, what does a
//! given instant look like as a date, and wake me later. Windows can answer all
//! three, but never in the shape Linux uses, so each entry point is a real query
//! against a real host source followed by a reshaping that is documented at the
//! point it happens. Nothing in this module reports a plausible-looking number in
//! place of one it could have asked for.
//!
//! Four divergences are large enough to state up front.
//!
//! **Elapsed clocks use Windows interrupt-time sources.** The unbiased clock
//! excludes suspend for `CLOCK_MONOTONIC` and `CLOCK_MONOTONIC_RAW`; the biased
//! clock includes suspend for `CLOCK_BOOTTIME`. Time namespaces apply their
//! separate monotonic and boot offsets. Realtime remains the host wall clock.
//!
//! **`tm_zone` reports a numeric zone rather than a Windows zone name.** Windows
//! names a zone `"Eastern Standard Time"`, which is not an abbreviation and is not
//! what any caller parsing `%Z` expects to find where `EST` belongs. Manufacturing
//! `EST` from it would be invention, and there is no table on the host mapping one
//! to the other. `%Z` therefore reports the ISO-style numeric form, `+09` or
//! `-0430`, which is what glibc itself prints for a zone it has no abbreviation
//! for. A caller who needs a real abbreviation can set `TZ`, which this module
//! parses and honours; see [`tz_from_environment`].
//!
//! **Setting the clock is attempted, never faked.** `clock_settime`,
//! `settimeofday` and `adjtimex`'s write modes call the real `SetSystemTime` or
//! `SetSystemTimeAdjustment` and report the host's verdict. Without
//! `SE_SYSTEMTIME_NAME` in the process token that verdict is a refusal, which
//! surfaces as `EPERM` — the same errno Linux gives an unprivileged caller, and a
//! condition every caller already handles.
//!
//! **`SIGALRM` is delivered through the existing signal path, not a new one.**
//! `alarm` and `setitimer` arm a timer thread that calls
//! `kinakaze_vfs::signal::raise_signal`, exactly as `kill` and `raise` do. The
//! consequence is described in full at [`TIMER`], and it is a real limitation
//! rather than a detail: the pending signal runs its handler when the guest next
//! reaches a delivery point, so a guest spinning in pure computation sees the
//! alarm late. Linux would interrupt it mid-instruction. Nothing available to a
//! user-mode library can.

use core::ffi::{CStr, c_char, c_int, c_uint, c_void};
use core::ptr;
pub(crate) mod zone_globals;

use std::collections::HashMap;
#[cfg(test)]
use std::ffi::CString;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use kinakaze_abi::{Long, Time};
use kinakaze_vfs::signal::{self, Delivery, SIGALRM};
use kinakaze_vfs::{EFAULT, EINTR, EINVAL, ENOSYS, EPERM};

// Win32 entry points this module queries directly.
//
// Declared here rather than imported from `windows-sys` because the crate's
// enabled feature set covers `Win32_System_Threading` only, while these names
// live in `Win32_System_SystemInformation` and `Win32_System_Time`. Declaring the
// handful of signatures needed keeps the module self-contained instead of
// widening a dependency shared with every other module in this crate.
//
// Each signature matches the Windows headers: `BOOL` is `i32`, a `DWORD` is
// `u32`, and a `LARGE_INTEGER` out-parameter is a single `i64`.
#[link(name = "kernel32")]
unsafe extern "system" {
    /// `GetSystemTimePreciseAsFileTime`: wall-clock UTC at the best resolution
    /// the host offers, which is the FILETIME tick of 100 ns.
    fn GetSystemTimePreciseAsFileTime(time: *mut FileTime);
    /// `GetSystemTimeAsFileTime`: the same clock read from the cached tick, which
    /// is what the `_COARSE` clocks want — no interpolation, no serialization.
    fn GetSystemTimeAsFileTime(time: *mut FileTime);
    /// `QueryPerformanceCounter`: the monotonic hardware counter. Documented as
    /// never failing on Windows XP or later.
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
    /// `QueryPerformanceFrequency`: that counter's ticks per second, fixed at
    /// boot.
    fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
    /// `GetTickCount64`: milliseconds since boot, from the same interrupt tick
    /// the coarse clocks read.
    fn GetTickCount64() -> u64;
    /// `GetSystemTimeAdjustment`: the increment added to the system clock on each
    /// interrupt, in 100 ns units, and whether periodic adjustment is disabled.
    /// This is the real granularity of the wall clock and of CPU accounting.
    fn GetSystemTimeAdjustment(
        adjustment: *mut u32,
        increment: *mut u32,
        disabled: *mut i32,
    ) -> i32;
    /// `SetSystemTimeAdjustment`: changes the per-interrupt increment, which is
    /// the host facility `adjtimex`'s frequency mode maps onto.
    fn SetSystemTimeAdjustment(adjustment: u32, disabled: i32) -> i32;
    /// `SetSystemTime`: steps the wall clock. Requires `SE_SYSTEMTIME_NAME`.
    fn SetSystemTime(time: *const SystemTime) -> i32;
    /// `GetTimeZoneInformation`: the zone rules in force now.
    fn GetTimeZoneInformation(information: *mut TimeZoneInformation) -> u32;
    /// `GetTimeZoneInformationForYear`: the zone rules for one specific year.
    ///
    /// Needed rather than the call above because a zone's rules change between
    /// years — the United States moved its DST dates in 2007 — and formatting a
    /// timestamp from 2005 with today's rules puts it an hour out.
    fn GetTimeZoneInformationForYear(
        year: u16,
        dynamic: *const c_void,
        information: *mut TimeZoneInformation,
    ) -> i32;
    /// `GetProcessTimes`: cumulative kernel and user time for this process.
    fn GetProcessTimes(
        process: *mut c_void,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    /// `GetThreadTimes`: the same four figures for one thread.
    fn GetThreadTimes(
        thread: *mut c_void,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    /// `GetCurrentProcess`: the pseudo-handle for this process.
    fn GetCurrentProcess() -> *mut c_void;
    /// `GetCurrentThread`: the pseudo-handle for this thread.
    fn GetCurrentThread() -> *mut c_void;
    /// `GetLastError`: the failure reason for the call just made.
    fn GetLastError() -> u32;
}

/// `ERROR_PRIVILEGE_NOT_HELD`, what `SetSystemTime` reports without the right.
const ERROR_PRIVILEGE_NOT_HELD: u32 = 1314;
/// `ERROR_ACCESS_DENIED`, the other refusal the same calls produce.
const ERROR_ACCESS_DENIED: u32 = 5;
/// `ERROR_INVALID_PARAMETER`, which a malformed `SYSTEMTIME` produces.
const ERROR_INVALID_PARAMETER: u32 = 87;

/// `FILETIME`, a count of 100-nanosecond ticks split across two 32-bit words.
///
/// The split is not cosmetic: the struct is 4-byte aligned, so it cannot simply be
/// declared as a `u64` without changing its alignment.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FileTime {
    low: u32,
    high: u32,
}

/// Seconds between the FILETIME epoch (1601-01-01) and the Unix epoch.
///
/// 11644473600 = 369 years of which 89 are leap, times 86400.
const FILETIME_EPOCH_OFFSET: i64 = 11_644_473_600;

/// FILETIME ticks per second: the unit is 100 ns.
const TICKS_PER_SECOND: i64 = 10_000_000;
/// Nanoseconds in one FILETIME tick.
const NANOSECONDS_PER_TICK: i64 = 100;

impl FileTime {
    /// Reassembles the split halves into the tick count they encode.
    fn ticks(self) -> u64 {
        (u64::from(self.high) << 32) | u64::from(self.low)
    }

    /// Reinterprets an absolute FILETIME as a Unix instant.
    ///
    /// Instants before 1970 produce a negative result, which is correct: Linux
    /// `time_t` is signed and callers do handle pre-epoch timestamps.
    fn to_unix(self) -> (i64, i64) {
        let ticks = self.ticks() as i64;
        let seconds = ticks.div_euclid(TICKS_PER_SECOND) - FILETIME_EPOCH_OFFSET;
        let nanoseconds = ticks.rem_euclid(TICKS_PER_SECOND) * NANOSECONDS_PER_TICK;
        (seconds, nanoseconds)
    }

    /// Reinterprets a FILETIME used as a *duration*, as CPU times are.
    ///
    /// No epoch shift applies here; the value is already an interval.
    fn to_duration(self) -> (i64, i64) {
        let ticks = self.ticks() as i64;
        (
            ticks / TICKS_PER_SECOND,
            (ticks % TICKS_PER_SECOND) * NANOSECONDS_PER_TICK,
        )
    }
}

/// `SYSTEMTIME`, the calendar form Windows uses for `SetSystemTime` and for the
/// transition dates inside `TIME_ZONE_INFORMATION`.
///
/// Eight 16-bit fields, 16 bytes. `day_of_week` is 0 for Sunday.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

/// `TIME_ZONE_INFORMATION`.
///
/// The three biases are in **minutes west of UTC**, the opposite sign to
/// `tm_gmtoff`. Windows defines `UTC = local + bias`, so the east-positive offset
/// a `struct tm` carries is the negation. Getting this backwards puts every
/// timestamp out by twice the offset, which is why the conversion happens in
/// exactly one place, [`ZoneRules::offset_at`].
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TimeZoneInformation {
    /// Minutes west of UTC for the zone as a whole.
    bias: i32,
    /// The standard-time name, as a wide string. Not an abbreviation; see the
    /// module header for why it is not used for `%Z`.
    standard_name: [u16; 32],
    /// When standard time resumes. A zero `month` means the zone has no DST.
    standard_date: SystemTime,
    /// Extra minutes west during standard time, conventionally 0.
    standard_bias: i32,
    daylight_name: [u16; 32],
    /// When daylight time begins.
    daylight_date: SystemTime,
    /// Extra minutes west during daylight time, conventionally -60.
    daylight_bias: i32,
}

/// `TIME_ZONE_ID_INVALID`, the failure return from `GetTimeZoneInformation`.
const TIME_ZONE_ID_INVALID: u32 = u32::MAX;

// ---------------------------------------------------------------------------
// Linux x86_64 ABI types.
//
// BusyBox was compiled against real glibc headers, so every layout below is
// contractual: guest code computes these field offsets at its own compile time
// and reads them directly. A wrong offset is not a compile error here, it is
// silent memory corruption in the guest, so each layout is stated in a comment
// and pinned by an `offset_of!` assertion in the test module.
//
// `time_t`, `clock_t` and `suseconds_t` are all `long` on Linux, which is 64-bit.
// They are spelled `i64`/`Time` rather than `core::ffi::c_long`, which on Windows
// MSVC is 32-bit and would deliver half a value into the register the guest reads.
// ---------------------------------------------------------------------------

/// The Linux `struct timespec`: `{ i64 tv_sec; i64 tv_nsec; }`, 16 bytes.
#[repr(C)]
#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub struct TimeSpec {
    pub tv_sec: Time,
    pub tv_nsec: Long,
}

/// The Linux `struct timeval`: `{ i64 tv_sec; i64 tv_usec; }`, 16 bytes.
///
/// `tv_usec` is `suseconds_t`, which is `long` and therefore 64-bit — not the
/// 32-bit field it is on some other platforms.
#[repr(C)]
#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub struct TimeVal {
    pub tv_sec: Time,
    pub tv_usec: Long,
}

/// The Linux x86_64 `struct tm`.
///
/// Nine `int` fields occupy bytes 0..36. `tm_gmtoff` is a `long`, so it needs
/// 8-byte alignment and the compiler inserts **4 bytes of padding at offset 36**,
/// placing `tm_gmtoff` at 40 and `tm_zone` at 48. The struct is therefore
/// **56 bytes**, not 48: forgetting the padding shifts both trailing fields and
/// makes a guest read `tm_gmtoff` out of the middle of two other values.
///
/// The last two fields are the BSD/glibc extension rather than C89. They are not
/// optional here — BusyBox `date` reads `tm_gmtoff` directly to print `%z`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Tm {
    /// Seconds, 0..=60. The 61st second exists for a leap second.
    pub tm_sec: c_int,
    pub tm_min: c_int,
    pub tm_hour: c_int,
    /// Day of month, 1..=31.
    pub tm_mday: c_int,
    /// Month, **0..=11**. January is 0.
    pub tm_mon: c_int,
    /// Year **less 1900**.
    pub tm_year: c_int,
    /// Day of week, 0..=6, Sunday first. Output-only for `mktime`.
    pub tm_wday: c_int,
    /// Day of year, **0..=365**. Zero-based, unlike `%j` which is one-based.
    pub tm_yday: c_int,
    /// Positive in DST, zero outside it, negative to ask for a determination.
    pub tm_isdst: c_int,
    /// Seconds **east** of UTC — the opposite sign to a POSIX `TZ` offset.
    pub tm_gmtoff: Long,
    /// The zone abbreviation. Points to storage that outlives the call.
    pub tm_zone: *const c_char,
}

impl Default for Tm {
    fn default() -> Self {
        Self {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 1,
            tm_mon: 0,
            tm_year: 70,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: 0,
            tm_gmtoff: 0,
            tm_zone: ptr::null(),
        }
    }
}

/// The Linux `struct itimerval`: two `timeval`s, 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ItimerVal {
    /// Reload value. Zero makes the timer one-shot.
    pub it_interval: TimeVal,
    /// Time until the next expiry. Zero disarms the timer.
    pub it_value: TimeVal,
}

/// The Linux `struct tms`: four `clock_t`, so four `i64`, 32 bytes.
///
/// Every field counts **clock ticks**, not seconds. See [`kinakaze_abi_times`].
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Tms {
    pub tms_utime: Time,
    pub tms_stime: Time,
    pub tms_cutime: Time,
    pub tms_cstime: Time,
}

/// The Linux `struct timex`, as `adjtimex` takes it.
///
/// The layout is the kernel's `__kernel_timex`: a `modes` int, 4 bytes of padding
/// to align the `long` that follows, eleven `long`s with a `timeval` embedded
/// between the sixth and seventh, then three more `long`s and a long tail of
/// reserved space. The whole struct is **208 bytes** and the trailing padding is
/// part of the ABI, so it is declared rather than omitted.
///
/// Not modelled: this layer has no phase-locked loop, so `maxerror`, `esterror`,
/// `constant`, `precision`, `tolerance`, `ppsfreq`, `jitter`, `shift`, `stabil`,
/// `jitcnt`, `calcnt`, `errcnt` and `stbcnt` are reported as zero, and the write
/// modes that would set them are refused rather than accepted and forgotten.
/// [`kinakaze_abi_adjtimex`] documents exactly which modes act.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Timex {
    /// Which fields the caller is asking to change; a bitmask of `ADJ_*`.
    pub modes: c_int,
    /// Padding the compiler would insert anyway, named so the layout is visible.
    pub _pad0: c_int,
    pub offset: Long,
    pub freq: Long,
    pub maxerror: Long,
    pub esterror: Long,
    pub status: c_int,
    pub _pad1: c_int,
    pub constant: Long,
    pub precision: Long,
    pub tolerance: Long,
    pub time: TimeVal,
    pub tick: Long,
    pub ppsfreq: Long,
    pub jitter: Long,
    pub shift: c_int,
    pub _pad2: c_int,
    pub stabil: Long,
    pub jitcnt: Long,
    pub calcnt: Long,
    pub errcnt: Long,
    pub stbcnt: Long,
    pub tai: c_int,
    /// The kernel's trailing reserved space: eleven ints of padding.
    pub _reserved: [c_int; 11],
}

/// Clock identifiers. These integers are ABI — the guest passes the number its
/// own headers assigned, so they cannot be renumbered.
pub const CLOCK_REALTIME: c_int = 0;
pub const CLOCK_MONOTONIC: c_int = 1;
pub const CLOCK_PROCESS_CPUTIME_ID: c_int = 2;
pub const CLOCK_THREAD_CPUTIME_ID: c_int = 3;
pub const CLOCK_MONOTONIC_RAW: c_int = 4;
pub const CLOCK_REALTIME_COARSE: c_int = 5;
pub const CLOCK_MONOTONIC_COARSE: c_int = 6;
pub const CLOCK_BOOTTIME: c_int = 7;

/// `TIMER_ABSTIME`, the `clock_nanosleep` flag naming an absolute deadline.
pub const TIMER_ABSTIME: c_int = 1;

/// `setitimer` timer kinds.
pub const ITIMER_REAL: c_int = 0;
pub const ITIMER_VIRTUAL: c_int = 1;
pub const ITIMER_PROF: c_int = 2;

/// Clock ticks per second, Linux's `USER_HZ`.
///
/// Fixed at 100 on Linux, and `sysconf(_SC_CLK_TCK)` in `userdb.rs` reports 100.
/// The two must agree: a caller that divides a `times()` result by what `sysconf`
/// told it gets a figure wrong by exactly the ratio if they drift apart.
const CLOCKS_PER_SECOND: i64 = 100;

// ---------------------------------------------------------------------------
// Civil-date arithmetic.
//
// The conversion between a calendar date and a day number is written out here
// rather than routed through `SystemTimeToFileTime`. That is deliberate: the
// Windows call validates its input and rejects a month of 13 or a day of 0, and
// those out-of-range values are precisely what `mktime` is required to normalise.
// Using the host call would reject exactly the inputs callers depend on — BusyBox
// `date -d` builds them on purpose.
//
// The algorithm shifts the year to start in March, which moves the leap day to
// the end of the year and removes every special case for February. It is exact for
// the whole range of a 64-bit day count and needs no lookup table.
// ---------------------------------------------------------------------------

/// Seconds in one day.
const SECONDS_PER_DAY: i64 = 86_400;
/// The weekday of 1970-01-01, which was a Thursday. `tm_wday` counts from Sunday.
const EPOCH_WEEKDAY: i64 = 4;

/// Days from 1970-01-01 to `year-month-day`, proleptic Gregorian.
///
/// `year` is the full year, `month` is 1..=12 and `day` is 1..=31. Values outside
/// those ranges are not rejected — the arithmetic extends smoothly through them,
/// which is what makes normalisation fall out for free.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // March-based years: the leap day lands last, so no month needs a special
    // case and the length of February never enters the arithmetic.
    let year = if month <= 2 { year - 1 } else { year };
    // The era is a 400-year cycle, which is the Gregorian repeat period.
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    // Day of the March-based year. The magic 153/5 is the exact closed form of
    // the 31/30/31/30/31 month-length pattern the March-based order produces.
    let month_shifted = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_shifted + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    // 719468 shifts the origin from 0000-03-01 to 1970-01-01.
    era * 146_097 + day_of_era - 719_468
}

/// The inverse of [`days_from_civil`]: `(year, month, day)` for a day number.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    // The three correction terms remove the century and 400-year leap rules.
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_shifted + 2) / 5 + 1;
    let month = if month_shifted < 10 {
        month_shifted + 3
    } else {
        month_shifted - 9
    };
    // The March-based year rolls over to the next calendar year in January.
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

/// Whether `year` is a leap year in the proleptic Gregorian calendar.
fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Converts a broken-down date and time to seconds since the epoch, treating the
/// fields as if they named a UTC instant.
///
/// Out-of-range fields are carried rather than rejected, so a `tm_mon` of 12 is
/// January of the next year and a `tm_mday` of 0 is the last day of the previous
/// month. That is the documented `mktime` behaviour.
fn seconds_from_fields(
    year: i64,
    month_zero_based: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
) -> i64 {
    // The month is normalised first because the day count needs a month in 1..=12
    // to index; the remaining fields carry through the multiplication naturally.
    let year = year + month_zero_based.div_euclid(12);
    let month = month_zero_based.rem_euclid(12) + 1;
    let days = days_from_civil(year, month, day);
    days * SECONDS_PER_DAY + hour * 3600 + minute * 60 + second
}

/// Fills the calendar fields of `tm` from a count of seconds since the epoch.
///
/// The zone fields are left untouched: the caller decides whether the instant is
/// being described in UTC or in local time, and sets `tm_gmtoff`, `tm_zone` and
/// `tm_isdst` accordingly.
fn fields_from_seconds(seconds: i64, tm: &mut Tm) {
    // Euclidean division, so a pre-epoch instant produces a positive
    // seconds-of-day and a more-negative day number rather than two negatives.
    let days = seconds.div_euclid(SECONDS_PER_DAY);
    let rest = seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);

    tm.tm_sec = (rest % 60) as c_int;
    tm.tm_min = ((rest / 60) % 60) as c_int;
    tm.tm_hour = (rest / 3600) as c_int;
    tm.tm_mday = day as c_int;
    tm.tm_mon = (month - 1) as c_int;
    tm.tm_year = (year - 1900) as c_int;
    // The weekday cycle has no exceptions, so it comes straight off the day
    // number. `rem_euclid` keeps it in 0..=6 for pre-epoch dates too.
    tm.tm_wday = ((days + EPOCH_WEEKDAY).rem_euclid(7)) as c_int;
    // `tm_yday` is zero-based, so January 1st is 0.
    tm.tm_yday = (days - days_from_civil(year, 1, 1)) as c_int;
}

/// Reads the calendar fields of a `tm` as `i64`, ignoring the zone fields.
fn fields_of(tm: &Tm) -> (i64, i64, i64, i64, i64, i64) {
    (
        i64::from(tm.tm_year) + 1900,
        i64::from(tm.tm_mon),
        i64::from(tm.tm_mday),
        i64::from(tm.tm_hour),
        i64::from(tm.tm_min),
        i64::from(tm.tm_sec),
    )
}

/// Days in `month` of `year`, with `month` 1..=12.
fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        // Not reachable through the callers, which all normalise first, but a
        // total function is better than a panic inside a libc entry point.
        _ => 30,
    }
}

/// The day of month of the `week`th `weekday` of `month`.
///
/// `weekday` is 0 for Sunday. `week` is 1..=5, where **5 means "the last one"**
/// and is pulled back to the fourth when the month has only four. Both the POSIX
/// `Mm.w.d` rule and Windows' `TIME_ZONE_INFORMATION` use this encoding.
fn nth_weekday_of_month(year: i64, month: i64, week: i64, weekday: i64) -> i64 {
    let first = days_from_civil(year, month, 1);
    let first_weekday = (first + EPOCH_WEEKDAY).rem_euclid(7);
    let shift = (weekday - first_weekday).rem_euclid(7);
    let mut day = 1 + shift + (week - 1) * 7;
    let length = days_in_month(year, month);
    // `week == 5` overshoots in most months, and so does the 5th occurrence of a
    // weekday that only happens four times.
    while day > length {
        day -= 7;
    }
    day
}

// ---------------------------------------------------------------------------
// Time zones.
//
// Two sources, in priority order: a POSIX `TZ` string when the guest set one, and
// the Windows zone otherwise. `TZ` wins because a guest that sets it is making an
// explicit request, and because it is the only way to obtain a real zone
// abbreviation on this host — see the module header.
//
// The sign convention is the trap here and it is worth restating. A POSIX `TZ`
// offset is **west-positive**: `EST5EDT` means five hours *behind* UTC. Windows'
// biases are west-positive too. `tm_gmtoff` is **east-positive**. Every conversion
// between them is a negation, and they are confined to `parse_offset` and
// `host_offset_at` so there is one place each to check.
// ---------------------------------------------------------------------------

/// A zone abbreviation, stored inline.
///
/// POSIX bounds an abbreviation at `TZNAME_MAX`, which is 6 on Linux, and the
/// numeric fallback this module generates is at most 7 bytes. The inline buffer
/// avoids an allocation on a path `localtime` takes on every call.
#[derive(Clone, Copy, Eq, PartialEq)]
struct ZoneAbbrev {
    bytes: [u8; 16],
    length: u8,
}

impl ZoneAbbrev {
    /// Builds an abbreviation from `text`, truncating at the inline capacity.
    fn new(text: &str) -> Self {
        let mut bytes = [0u8; 16];
        let length = text.len().min(bytes.len());
        bytes[..length].copy_from_slice(&text.as_bytes()[..length]);
        Self {
            bytes,
            length: length as u8,
        }
    }

    /// The ISO-style numeric form for `offset` seconds east of UTC.
    ///
    /// `+09`, or `+0930` when the offset is not a whole hour, or `+093015` in the
    /// rare case of a sub-minute offset. This is what glibc prints for a zone with
    /// no abbreviation, and it is unambiguous where a Windows zone name would not
    /// be an abbreviation at all.
    fn numeric(offset: i64) -> Self {
        let sign = if offset < 0 { '-' } else { '+' };
        let magnitude = offset.abs();
        let (hours, minutes, seconds) = (magnitude / 3600, (magnitude / 60) % 60, magnitude % 60);
        let text = if seconds != 0 {
            format!("{sign}{hours:02}{minutes:02}{seconds:02}")
        } else if minutes != 0 {
            format!("{sign}{hours:02}{minutes:02}")
        } else {
            format!("{sign}{hours:02}")
        };
        Self::new(&text)
    }

    fn as_str(&self) -> &str {
        // Every construction path writes UTF-8: either a `&str` slice or ASCII
        // digits. `from_utf8_lossy` keeps the function total regardless.
        core::str::from_utf8(&self.bytes[..self.length as usize]).unwrap_or("")
    }
}

/// When a daylight-saving transition happens, in local wall-clock terms.
#[derive(Clone, Copy)]
enum Transition {
    /// `Mm.w.d[/time]`: the `week`th `weekday` of `month`.
    MonthWeekDay {
        month: i64,
        week: i64,
        weekday: i64,
        /// Seconds after local midnight.
        time: i64,
    },
    /// `Jn[/time]`: day `n` of the year, 1..=365, **never counting February 29**.
    JulianNoLeap { day: i64, time: i64 },
    /// `n[/time]`: day `n`, 0..=365, counting February 29.
    ZeroBasedDay { day: i64, time: i64 },
}

impl Transition {
    /// The instant this transition occurs in `year`, as local wall-clock seconds
    /// since the epoch — that is, the value `timegm` would give the local fields.
    ///
    /// POSIX specifies the transition time in the local time in force *before* the
    /// change, so the caller subtracts the appropriate offset to reach UTC.
    fn local_seconds(self, year: i64) -> i64 {
        let start_of_year = days_from_civil(year, 1, 1);
        match self {
            Self::MonthWeekDay {
                month,
                week,
                weekday,
                time,
            } => {
                let day = nth_weekday_of_month(year, month, week, weekday);
                days_from_civil(year, month, day) * SECONDS_PER_DAY + time
            }
            Self::JulianNoLeap { day, time } => {
                // Day 60 is March 1st in every year, leap or not, because
                // February 29th is not assigned a number in this form.
                let day = if is_leap_year(year) && day >= 60 {
                    day + 1
                } else {
                    day
                };
                (start_of_year + day - 1) * SECONDS_PER_DAY + time
            }
            Self::ZeroBasedDay { day, time } => (start_of_year + day) * SECONDS_PER_DAY + time,
        }
    }
}

/// The rules a zone follows.
#[derive(Clone, Copy)]
enum Rules {
    /// One offset, all year. `TZ=UTC0`, `TZ=EST5`, `TZ=GMT`.
    Fixed { offset: i64, abbrev: ZoneAbbrev },
    /// A standard/daylight pair with explicit transitions, from a POSIX `TZ`.
    Posix {
        standard: i64,
        standard_abbrev: ZoneAbbrev,
        daylight: i64,
        daylight_abbrev: ZoneAbbrev,
        start: Transition,
        end: Transition,
    },
    /// Whatever Windows reports, evaluated against the rules for each year.
    Host,
}

/// A zone's state at one instant.
#[derive(Clone, Copy)]
struct Offset {
    /// Seconds **east** of UTC, the `tm_gmtoff` convention.
    seconds: i64,
    /// Whether daylight saving is in force.
    daylight: bool,
    abbrev: ZoneAbbrev,
}

/// Reads the zone rules Windows reports for `year`.
///
/// `GetTimeZoneInformationForYear` is preferred over `GetTimeZoneInformation`
/// because a zone's rules change between years — the United States moved its DST
/// dates in 2007 — and evaluating a 2005 timestamp against today's rules puts it
/// an hour out for several weeks of the year.
fn zone_information_for(year: i64) -> Option<TimeZoneInformation> {
    let mut information = TimeZoneInformation::default();
    // The parameter is a `USHORT`, so a year outside its range is clamped rather
    // than wrapped. The rules for a year that far out are not knowable anyway, and
    // the nearest representable year is the closest available answer.
    let clamped = year.clamp(1601, 30827) as u16;
    // SAFETY: `information` is a writable local of the right type and a null
    // `dynamic` requests the current zone, which is the documented default.
    if unsafe { GetTimeZoneInformationForYear(clamped, ptr::null(), &raw mut information) } != 0 {
        return Some(information);
    }
    // The per-year call is unavailable or failed; the current rules are the best
    // remaining answer and are correct for any year the rules did not change in.
    let mut information = TimeZoneInformation::default();
    // SAFETY: as above.
    if unsafe { GetTimeZoneInformation(&raw mut information) } == TIME_ZONE_ID_INVALID {
        return None;
    }
    Some(information)
}

/// The instant a `SYSTEMTIME` transition rule names, as local wall-clock seconds.
fn transition_local_seconds(date: &SystemTime, year: i64) -> i64 {
    let time = i64::from(date.hour) * 3600 + i64::from(date.minute) * 60 + i64::from(date.second);
    let days = if date.year == 0 {
        // The recurring form: `day` is a week number 1..=5 and `day_of_week` is
        // 0 for Sunday, exactly the POSIX `Mm.w.d` encoding.
        let day = nth_weekday_of_month(
            year,
            i64::from(date.month),
            i64::from(date.day),
            i64::from(date.day_of_week),
        );
        days_from_civil(year, i64::from(date.month), day)
    } else {
        // The absolute form, which Windows uses for a one-off rule change.
        days_from_civil(
            i64::from(date.year),
            i64::from(date.month),
            i64::from(date.day),
        )
    };
    days * SECONDS_PER_DAY + time
}

/// The host zone's offset at the UTC instant `utc`.
///
/// The year used to select the rules is taken from `utc` rather than from local
/// time. Within a few hours of January 1st those can differ, so a zone whose rules
/// changed exactly at that boundary would be evaluated against the neighbouring
/// year's. No real zone does that: rule changes are legislated months ahead and
/// take effect at a DST transition, never within hours of midnight UTC.
fn host_offset_at(utc: i64) -> Offset {
    let (year, _, _) = civil_from_days(utc.div_euclid(SECONDS_PER_DAY));
    let Some(information) = zone_information_for(year) else {
        // The zone cannot be read at all. UTC is the honest fallback: it is a real
        // offset rather than a guess at the machine's, and it is what a Linux host
        // with no `/etc/localtime` reports.
        return Offset {
            seconds: 0,
            daylight: false,
            abbrev: ZoneAbbrev::new("UTC"),
        };
    };

    // Windows biases are minutes west; `tm_gmtoff` is seconds east.
    let standard = -i64::from(information.bias + information.standard_bias) * 60;
    let daylight = -i64::from(information.bias + information.daylight_bias) * 60;

    // A zero month in either transition means the zone observes no DST.
    if information.standard_date.month == 0 || information.daylight_date.month == 0 {
        return Offset {
            seconds: standard,
            daylight: false,
            abbrev: ZoneAbbrev::numeric(standard),
        };
    }

    // Each rule is expressed in the local time in force before it fires, so the
    // start converts with the standard offset and the end with the daylight one.
    let start = transition_local_seconds(&information.daylight_date, year) - standard;
    let end = transition_local_seconds(&information.standard_date, year) - daylight;
    let in_daylight = if start <= end {
        // Northern hemisphere: the DST interval sits inside the year.
        utc >= start && utc < end
    } else {
        // Southern hemisphere: it wraps the year boundary, so the test inverts.
        utc >= start || utc < end
    };

    let seconds = if in_daylight { daylight } else { standard };
    Offset {
        seconds,
        daylight: in_daylight,
        abbrev: ZoneAbbrev::numeric(seconds),
    }
}

/// Splits a leading zone abbreviation off `text`.
///
/// Two spellings exist. The bare form runs until a sign or a digit, and POSIX
/// requires at least three characters. The angle-bracket form `<+04>` exists
/// precisely so an abbreviation may contain digits or a sign.
fn parse_abbrev(text: &str) -> Option<(ZoneAbbrev, &str)> {
    if let Some(rest) = text.strip_prefix('<') {
        let end = rest.find('>')?;
        let (name, rest) = rest.split_at(end);
        if name.is_empty() {
            return None;
        }
        // Skip the '>' itself.
        return Some((ZoneAbbrev::new(name), &rest[1..]));
    }
    // The bare form is alphabetic only. Scanning to the first digit or sign
    // instead would swallow the comma in `EST5EDT,M3.2.0,M11.1.0` and take the
    // abbreviation as `EDT,M`, which then fails the rule parse and silently drops
    // the whole zone back to the host's.
    let end = text
        .find(|character: char| !character.is_ascii_alphabetic())
        .unwrap_or(text.len());
    let (name, rest) = text.split_at(end);
    // Fewer than three characters is not a legal abbreviation, and accepting one
    // would let a malformed string parse as a zone with a strange name instead of
    // falling back to the host zone.
    if name.len() < 3 {
        return None;
    }
    Some((ZoneAbbrev::new(name), rest))
}

/// Parses a leading run of digits, bounded by `limit`.
fn parse_number(text: &str, limit: i64) -> Option<(i64, &str)> {
    let end = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    let value: i64 = text[..end].parse().ok()?;
    if value > limit {
        return None;
    }
    Some((value, &text[end..]))
}

/// Parses a POSIX `TZ` offset, returning **seconds east of UTC**.
///
/// The form is `[+|-]hh[:mm[:ss]]`. The sign convention is inverted on the way
/// out: POSIX writes the offset as time to *add to local time to reach UTC*, so
/// `EST5` is five hours west and `tm_gmtoff` for it is -18000. This negation is
/// the one every implementation gets wrong once.
fn parse_offset(text: &str) -> Option<(i64, &str)> {
    let (negate, rest) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    // POSIX allows up to 24, and glibc accepts the wider range a zone might need.
    let (hours, rest) = parse_number(rest, 167)?;
    let mut total = hours * 3600;
    let mut rest = rest;
    if let Some(after) = rest.strip_prefix(':') {
        let (minutes, after) = parse_number(after, 59)?;
        total += minutes * 60;
        rest = after;
        if let Some(after) = rest.strip_prefix(':') {
            let (seconds, after) = parse_number(after, 59)?;
            total += seconds;
            rest = after;
        }
    }
    // West-positive in, east-positive out.
    Some((if negate { total } else { -total }, rest))
}

/// The default transition time when a rule omits one: 02:00 local.
const DEFAULT_TRANSITION_TIME: i64 = 2 * 3600;

/// Parses one transition rule: `Mm.w.d`, `Jn` or `n`, each with an optional
/// `/time` suffix.
fn parse_transition(text: &str) -> Option<(Transition, &str)> {
    /// Parses the `/time` suffix, which may be negative or exceed 24 hours in
    /// the POSIX 2004 extension glibc implements.
    fn parse_time(text: &str) -> Option<(i64, &str)> {
        let Some(rest) = text.strip_prefix('/') else {
            return Some((DEFAULT_TRANSITION_TIME, text));
        };
        let (negate, rest) = match rest.as_bytes().first() {
            Some(b'-') => (true, &rest[1..]),
            Some(b'+') => (false, &rest[1..]),
            _ => (false, rest),
        };
        let (hours, rest) = parse_number(rest, 167)?;
        let mut total = hours * 3600;
        let mut rest = rest;
        if let Some(after) = rest.strip_prefix(':') {
            let (minutes, after) = parse_number(after, 59)?;
            total += minutes * 60;
            rest = after;
            if let Some(after) = rest.strip_prefix(':') {
                let (seconds, after) = parse_number(after, 59)?;
                total += seconds;
                rest = after;
            }
        }
        Some((if negate { -total } else { total }, rest))
    }

    if let Some(rest) = text.strip_prefix('M') {
        let (month, rest) = parse_number(rest, 12)?;
        if month < 1 {
            return None;
        }
        let rest = rest.strip_prefix('.')?;
        let (week, rest) = parse_number(rest, 5)?;
        if week < 1 {
            return None;
        }
        let rest = rest.strip_prefix('.')?;
        let (weekday, rest) = parse_number(rest, 6)?;
        let (time, rest) = parse_time(rest)?;
        return Some((
            Transition::MonthWeekDay {
                month,
                week,
                weekday,
                time,
            },
            rest,
        ));
    }
    if let Some(rest) = text.strip_prefix('J') {
        let (day, rest) = parse_number(rest, 365)?;
        if day < 1 {
            return None;
        }
        let (time, rest) = parse_time(rest)?;
        return Some((Transition::JulianNoLeap { day, time }, rest));
    }
    let (day, rest) = parse_number(text, 365)?;
    let (time, rest) = parse_time(rest)?;
    Some((Transition::ZeroBasedDay { day, time }, rest))
}

/// The United States rules, which glibc uses when `TZ` names a DST zone but gives
/// no transition rules.
///
/// `M3.2.0` is the second Sunday in March and `M11.1.0` the first in November.
/// This is a documented glibc fallback rather than a guess, and it is only reached
/// by a `TZ` that asked for DST without saying when.
const DEFAULT_DST_START: Transition = Transition::MonthWeekDay {
    month: 3,
    week: 2,
    weekday: 0,
    time: DEFAULT_TRANSITION_TIME,
};
const DEFAULT_DST_END: Transition = Transition::MonthWeekDay {
    month: 11,
    week: 1,
    weekday: 0,
    time: DEFAULT_TRANSITION_TIME,
};

/// Parses a POSIX `TZ` value into zone rules.
///
/// Handles `STD`, `STDoffset`, `STDoffsetDST`, `STDoffsetDST,start,end` and the
/// `<+04>` bracketed abbreviation form, with `Mm.w.d`, `Jn` and `n` transitions
/// each taking an optional `/time`.
///
/// **Not parsed:** the `:` prefix and any name containing `/`, which both name a
/// zoneinfo file — `TZ=:/etc/localtime` or `TZ=Europe/Berlin`. Those need the IANA
/// database, which does not exist on Windows and which no Win32 call can be asked
/// to interpret; there is no mapping from an IANA name to a Windows zone on the
/// host. Such a value falls back to the Windows zone, which is the most useful
/// answer available: it is the real local time of the machine, whereas guessing at
/// the requested zone's rules would be invention. `None` is returned so the caller
/// makes that fallback explicit.
fn parse_tz(value: &str) -> Option<Rules> {
    // An empty `TZ` means UTC, which POSIX specifies directly.
    if value.is_empty() {
        return Some(Rules::Fixed {
            offset: 0,
            abbrev: ZoneAbbrev::new("UTC"),
        });
    }
    // A leading colon names a zoneinfo file, which POSIX reserves for exactly that
    // purpose. Only that spelling is special-cased here: a `/` cannot be tested for
    // directly, because a POSIX rule uses it for the transition time and
    // `EST5EDT,M3.2.0/2,M11.1.0/2` is a perfectly ordinary value.
    //
    // An IANA name like `Europe/Berlin` needs no special case either — it falls out
    // of the grammar. `Europe` parses as an abbreviation and `/Berlin` is not an
    // offset, so the parse fails and the caller falls back to the host zone.
    if value.starts_with(':') {
        return None;
    }

    let (standard_abbrev, rest) = parse_abbrev(value)?;
    // A bare name with no offset is only meaningful for the zone names that mean
    // UTC. Every other bare name — `Japan`, `Poland`, `EST` — is a zoneinfo file
    // name whose offset lives in that file, and treating it as UTC under a
    // different label would put the clock out by the whole offset while looking
    // like it had worked. Those fall through to the host zone instead.
    if rest.is_empty() {
        const UNIVERSAL: [&str; 6] = ["UTC", "GMT", "UCT", "Zulu", "Universal", "Greenwich"];
        if UNIVERSAL
            .iter()
            .any(|name| name.eq_ignore_ascii_case(standard_abbrev.as_str()))
        {
            return Some(Rules::Fixed {
                offset: 0,
                abbrev: standard_abbrev,
            });
        }
        return None;
    }
    let (standard, rest) = parse_offset(rest)?;
    if rest.is_empty() {
        return Some(Rules::Fixed {
            offset: standard,
            abbrev: standard_abbrev,
        });
    }

    let (daylight_abbrev, rest) = parse_abbrev(rest)?;
    // An omitted DST offset means one hour east of standard, per POSIX.
    let (daylight, rest) = if rest.is_empty() || rest.starts_with(',') {
        (standard + 3600, rest)
    } else {
        parse_offset(rest)?
    };

    let (start, end) = if let Some(rest) = rest.strip_prefix(',') {
        let (start, rest) = parse_transition(rest)?;
        let rest = rest.strip_prefix(',')?;
        let (end, rest) = parse_transition(rest)?;
        // Trailing junk means the string was not understood, and falling back is
        // better than acting on half of it.
        if !rest.is_empty() {
            return None;
        }
        (start, end)
    } else if rest.is_empty() {
        (DEFAULT_DST_START, DEFAULT_DST_END)
    } else {
        return None;
    };

    Some(Rules::Posix {
        standard,
        standard_abbrev,
        daylight,
        daylight_abbrev,
        start,
        end,
    })
}

/// Reads `TZ` from the guest's environment.
///
/// The guest's `environ` block is the source of truth, not the host's environment:
/// a guest that calls `setenv("TZ", ...)` writes into that block, and reading the
/// host's copy would miss it entirely. The host environment is consulted only as a
/// fallback, which is what makes this work in unit tests where no guest has
/// published an `environ`.
fn tz_from_environment() -> Option<String> {
    // SAFETY: a null-terminated literal, and the returned pointer addresses the
    // environment entry, which stays valid until that entry is replaced.
    let value = unsafe { crate::process::kinakaze_abi_getenv(c"TZ".as_ptr()) };
    if !value.is_null() {
        // SAFETY: `getenv` returns a pointer into a null-terminated entry.
        if let Ok(text) = unsafe { CStr::from_ptr(value) }.to_str() {
            return Some(text.to_string());
        }
    }
    std::env::var("TZ").ok()
}

/// The rules in force, from `TZ` when it names a zone this module can parse and
/// from Windows otherwise.
///
/// `TZ` is re-read on every call rather than cached. POSIX requires `localtime` to
/// behave as if it called `tzset`, so a guest that changes `TZ` between two calls
/// must see the change on the second — caching would break that, and the parse is
/// a few dozen bytes of scanning against a Win32 call that dominates it anyway.
fn zone_rules() -> Rules {
    match tz_from_environment() {
        Some(value) => parse_tz(&value).unwrap_or(Rules::Host),
        None => Rules::Host,
    }
}

/// The zone's state at the UTC instant `utc`.
fn offset_at(rules: Rules, utc: i64) -> Offset {
    match rules {
        Rules::Fixed { offset, abbrev } => Offset {
            seconds: offset,
            daylight: false,
            abbrev,
        },
        Rules::Posix {
            standard,
            standard_abbrev,
            daylight,
            daylight_abbrev,
            start,
            end,
        } => {
            let (year, _, _) = civil_from_days(utc.div_euclid(SECONDS_PER_DAY));
            // As with the host rules, each transition is stated in the local time
            // in force immediately before it, so the offsets differ.
            let begins = start.local_seconds(year) - standard;
            let ends = end.local_seconds(year) - daylight;
            let in_daylight = if begins <= ends {
                utc >= begins && utc < ends
            } else {
                // The southern-hemisphere case, where DST spans New Year.
                utc >= begins || utc < ends
            };
            if in_daylight {
                Offset {
                    seconds: daylight,
                    daylight: true,
                    abbrev: daylight_abbrev,
                }
            } else {
                Offset {
                    seconds: standard,
                    daylight: false,
                    abbrev: standard_abbrev,
                }
            }
        }
        Rules::Host => host_offset_at(utc),
    }
}

/// The offset for a *local* wall-clock instant, which is the direction `mktime`
/// needs and the harder one.
///
/// The offset is needed to convert local time to UTC, but selecting the offset
/// needs the UTC instant, so the first guess is refined. Two passes suffice for
/// every real zone: the correction is at most the DST delta, which never moves the
/// instant across a second transition.
///
/// Around a transition the mapping is genuinely not one-to-one. In the fall-back
/// hour a local time names two instants, and `tm_isdst` is how the caller picks:
/// positive selects the daylight reading, zero the standard one. A negative
/// `tm_isdst` asks this code to choose, and it takes the earlier instant, which is
/// the DST one — matching glibc. In the spring-forward gap the local time names no
/// instant at all; the standard offset is used, which places the result an hour
/// after the requested wall time, again as glibc does.
fn offset_for_local(rules: Rules, local: i64, isdst: c_int) -> Offset {
    // First guess: treat the local value as if it were UTC. The error is bounded
    // by the offset itself, which is under 15 hours everywhere on Earth.
    let mut offset = offset_at(rules, local);
    for _ in 0..2 {
        let refined = offset_at(rules, local - offset.seconds);
        if refined.seconds == offset.seconds {
            break;
        }
        offset = refined;
    }

    // An explicit `tm_isdst` that disagrees with the converged answer selects the
    // other reading, which is what disambiguates the repeated hour.
    if isdst > 0 && !offset.daylight {
        let alternative = offset_at(rules, local - offset.seconds - 3600);
        if alternative.daylight {
            return alternative;
        }
    } else if isdst == 0 && offset.daylight {
        let alternative = offset_at(rules, local - offset.seconds + 3600);
        if !alternative.daylight {
            return alternative;
        }
    }
    offset
}

/// Publishes a zone abbreviation as a `const char *` the guest may hold.
///
/// `tm_zone` outlives the call that produced it — glibc points it at storage in
/// the C library — so the string cannot live in the `struct tm` or on the stack.
/// Each distinct abbreviation is interned once and leaked deliberately. The set is
/// grows only when the process requests a previously unseen abbreviation. Old
/// pointers must remain valid even after TZ changes and across guest fork.
fn published_abbrev(abbrev: ZoneAbbrev) -> *const c_char {
    static NAMES: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    let names = NAMES.get_or_init(|| Mutex::new(HashMap::new()));
    let text = abbrev.as_str();
    let Ok(mut names) = names.lock() else {
        // A poisoned lock means another thread panicked mid-insert. An empty
        // string is a valid `char *` and keeps this total; returning null would
        // fault a guest that prints `%Z`.
        return c"".as_ptr();
    };
    if let Some(&pointer) = names.get(text) {
        return pointer as *const c_char;
    }
    // Guest allocation survives fork; a Rust host-heap CString would leave
    // inherited tm_zone and tzname pointers dangling in the new native process.
    let pointer = unsafe { kinakaze_alloc::guest::malloc(text.len() + 1) }.cast::<u8>();
    if pointer.is_null() {
        return c"".as_ptr();
    }
    unsafe {
        ptr::copy_nonoverlapping(text.as_ptr(), pointer, text.len());
        pointer.add(text.len()).write(0);
    }
    let pointer = pointer as usize;
    names.insert(text.to_string(), pointer);
    pointer as *const c_char
}

/// Fills the zone fields of `tm` for the UTC instant `utc`.
fn apply_zone(tm: &mut Tm, offset: Offset) {
    tm.tm_gmtoff = offset.seconds;
    tm.tm_isdst = c_int::from(offset.daylight);
    tm.tm_zone = published_abbrev(offset.abbrev);
}

// ---------------------------------------------------------------------------
// Clocks.
//
// Every reading below comes from a real host source. The mapping, and the one
// place where Linux draws a distinction Windows does not, is in the module header.
// ---------------------------------------------------------------------------

/// Reads the wall clock. `precise` selects the interpolated read.
///
/// The coarse form is not a degraded stand-in: `GetSystemTimeAsFileTime` reads the
/// cached tick without serializing against the timer hardware, which is exactly
/// what Linux's `_COARSE` clocks offer and why callers ask for them.
fn read_realtime(precise: bool) -> (i64, i64) {
    let mut time = FileTime::default();
    // SAFETY: `time` is a writable local of the right type; neither call fails.
    unsafe {
        if precise {
            GetSystemTimePreciseAsFileTime(&raw mut time);
        } else {
            GetSystemTimeAsFileTime(&raw mut time);
        }
    }
    time.to_unix()
}

/// The performance counter's frequency, read once.
///
/// Fixed at boot on every supported Windows version, so caching it is not an
/// assumption about the hardware but a documented property of the API.
pub(crate) fn performance_frequency() -> i64 {
    // SAFETY: initialized by the libc provider CRT and immutable afterward.
    let frequency = unsafe { PERFORMANCE_FREQUENCY.0.get().read() };
    if frequency <= 0 {
        TICKS_PER_SECOND
    } else {
        frequency
    }
}

struct PerformanceFrequency(std::cell::UnsafeCell<i64>);

// SAFETY: the libc provider CRT is the sole writer before application threads.
unsafe impl Sync for PerformanceFrequency {}

static PERFORMANCE_FREQUENCY: PerformanceFrequency =
    PerformanceFrequency(std::cell::UnsafeCell::new(0));

extern "C" fn performance_frequency_initializer() {
    let mut frequency = 0i64;
    // SAFETY: `frequency` is a writable local; the call cannot fail on any
    // supported Windows version.
    unsafe { QueryPerformanceFrequency(&raw mut frequency) };
    // SAFETY: this runs in the provider CRT before application threads.
    unsafe { PERFORMANCE_FREQUENCY.0.get().write(frequency) };
}

#[used]
#[unsafe(link_section = ".CRT$XCU")]
static PERFORMANCE_FREQUENCY_INITIALIZER: extern "C" fn() = performance_frequency_initializer;

/// Reads the monotonic counter, as seconds and nanoseconds since boot.
fn read_monotonic() -> (i64, i64) {
    let mut count = 0i64;
    // SAFETY: `count` is a writable local; the call cannot fail on any supported
    // version, and a zero result would still be a valid, if useless, reading.
    if unsafe { QueryPerformanceCounter(&raw mut count) } == 0 {
        // The documented-impossible case. `GetTickCount64` is the fallback because
        // it measures the same thing at lower resolution, so the clock stays
        // monotonic rather than jumping.
        // SAFETY: no preconditions.
        let milliseconds = unsafe { GetTickCount64() } as i64;
        return (milliseconds / 1000, (milliseconds % 1000) * 1_000_000);
    }
    let frequency = performance_frequency();
    // The remainder is scaled before dividing so no precision is lost. The
    // intermediate is at most frequency * 1e9, which for a 10 MHz counter is 1e16
    // and well inside i64.
    (
        count / frequency,
        (count % frequency) * 1_000_000_000 / frequency,
    )
}

/// Sums a process's or thread's kernel and user time.
///
/// Linux's CPU-time clocks count both, so the two FILETIMEs are added rather than
/// one being reported alone.
fn read_cpu_time(thread: bool) -> Option<(i64, i64)> {
    let mut creation = FileTime::default();
    let mut exit = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    // SAFETY: both pseudo-handles are always valid for the calling process and
    // thread, and all four out-parameters are writable locals.
    let ok = unsafe {
        if thread {
            GetThreadTimes(
                GetCurrentThread(),
                &raw mut creation,
                &raw mut exit,
                &raw mut kernel,
                &raw mut user,
            )
        } else {
            GetProcessTimes(
                GetCurrentProcess(),
                &raw mut creation,
                &raw mut exit,
                &raw mut kernel,
                &raw mut user,
            )
        }
    };
    if ok == 0 {
        return None;
    }
    let total = kernel.ticks() + user.ticks();
    let combined = FileTime {
        low: total as u32,
        high: (total >> 32) as u32,
    };
    Some(combined.to_duration())
}

/// The system clock's real granularity, in nanoseconds.
///
/// Read from `GetSystemTimeAdjustment` rather than assumed. The figure is the
/// increment added to the clock on each timer interrupt, typically 156250 ns, and
/// it is the true resolution of both the coarse wall clock and CPU accounting —
/// which is why `clock_getres` reports it for those clocks instead of claiming the
/// 100 ns the FILETIME unit could express.
fn system_clock_granularity() -> i64 {
    let mut adjustment = 0u32;
    let mut increment = 0u32;
    let mut disabled = 0i32;
    // SAFETY: three writable locals of the documented types.
    let ok = unsafe {
        GetSystemTimeAdjustment(&raw mut adjustment, &raw mut increment, &raw mut disabled)
    };
    if ok == 0 || increment == 0 {
        // The call failed. One millisecond is the coarsest the interrupt period
        // has ever been, so it is a safe upper bound rather than a fabrication.
        return 1_000_000;
    }
    i64::from(increment) * NANOSECONDS_PER_TICK
}

/// Reads `clock` into `(seconds, nanoseconds)`, or reports the errno to use.
fn read_clock(clock: c_int) -> Result<(i64, i64), i32> {
    match clock {
        CLOCK_REALTIME | 8 => Ok(read_realtime(true)),
        CLOCK_REALTIME_COARSE => Ok(read_realtime(false)),
        // Windows unbiased/biased interrupt time preserves suspend semantics;
        // the namespace applies the corresponding immutable offset.
        CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_MONOTONIC_COARSE | CLOCK_BOOTTIME | 9 => {
            kinakaze_vfs::time_namespace::clock(clock)
        }
        CLOCK_PROCESS_CPUTIME_ID => read_cpu_time(false).ok_or(EINVAL),
        CLOCK_THREAD_CPUTIME_ID => read_cpu_time(true).ok_or(EINVAL),
        clock if clock < 0 && clock & 7 == 6 => thread_clock(clock),
        // An unknown clock id is EINVAL on Linux, including for the dynamic
        // per-process clocks this layer does not implement.
        _ => Err(EINVAL),
    }
}

/// `clock_gettime`.
///
/// # Safety
///
/// `value` must point at a writable `struct timespec`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_clock_gettime(
    clock: c_int,
    value: *mut TimeSpec,
) -> c_int {
    if value.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    match read_clock(clock) {
        Ok((seconds, nanoseconds)) => {
            // SAFETY: the caller guarantees a writable struct.
            unsafe {
                ptr::write(
                    value,
                    TimeSpec {
                        tv_sec: seconds,
                        tv_nsec: nanoseconds,
                    },
                );
            }
            0
        }
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// C11 calendar time uses Linux TIME_UTC (1), not a POSIX clock identifier.
///
/// # Safety
/// `value` must point to a writable `struct timespec` for a supported base.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_timespec_get(
    value: *mut TimeSpec,
    base: c_int,
) -> c_int {
    if base == 1 && unsafe { kinakaze_abi_clock_gettime(CLOCK_REALTIME, value) } == 0 {
        base
    } else {
        0
    }
}

/// C23 resolution of the C11 calendar time base.
///
/// # Safety
/// `value` must be null or point to a writable `struct timespec`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_timespec_getres(
    value: *mut TimeSpec,
    base: c_int,
) -> c_int {
    if base == 1 && unsafe { kinakaze_abi_clock_getres(CLOCK_REALTIME, value) } == 0 {
        base
    } else {
        0
    }
}

/// `clock_getres`, the granularity of `clock`.
///
/// Precise interrupt-time and wall-clock sources use 100 ns units; coarse and
/// CPU clocks use the system timer increment.
///
/// # Safety
///
/// `value` must be null or point at a writable `struct timespec`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_clock_getres(
    clock: c_int,
    value: *mut TimeSpec,
) -> c_int {
    let nanoseconds = match clock {
        // The precise wall clock is read straight from the hardware timer, so the
        // FILETIME unit is its real limit.
        CLOCK_REALTIME | 8 => NANOSECONDS_PER_TICK,
        CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_BOOTTIME | 9 => NANOSECONDS_PER_TICK,
        CLOCK_REALTIME_COARSE
        | CLOCK_MONOTONIC_COARSE
        | CLOCK_PROCESS_CPUTIME_ID
        | CLOCK_THREAD_CPUTIME_ID => system_clock_granularity(),
        id if id < 0 && id & 7 == 6 && thread_clock(id).is_ok() => system_clock_granularity(),
        _ => {
            crate::set_errno(EINVAL);
            return -1;
        }
    };
    // A null pointer is not an error: Linux accepts it as a bare "is this clock
    // supported" probe, and callers use it that way.
    if value.is_null() {
        return 0;
    }
    // SAFETY: the caller guarantees a writable struct.
    unsafe {
        ptr::write(
            value,
            TimeSpec {
                tv_sec: 0,
                tv_nsec: nanoseconds,
            },
        );
    }
    0
}

/// `time`, the seconds since the epoch.
///
/// The coarse clock is read rather than the precise one. `time` has one-second
/// resolution, so interpolating to 100 ns and then discarding all of it would be
/// pure cost, and this is a call BusyBox `date` makes on every invocation.
///
/// # Safety
///
/// `result` must be null or point at a writable `time_t`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_time(result: *mut Time) -> Time {
    let (seconds, _) = read_realtime(false);
    if !result.is_null() {
        // SAFETY: the caller guarantees a writable `time_t`.
        unsafe { ptr::write(result, seconds) };
    }
    seconds
}

/// `gettimeofday`.
///
/// The `timezone` argument is obsolete and Linux ignores it, writing nothing. It is
/// accepted and ignored here for the same reason: the struct describes a
/// pre-zoneinfo world, glibc has not filled it in for decades, and a caller that
/// passes it is passing a null pointer in practice.
///
/// # Safety
///
/// `value` must be null or point at a writable `struct timeval`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_gettimeofday(
    value: *mut TimeVal,
    _zone: *mut c_void,
) -> c_int {
    if value.is_null() {
        // Linux accepts a null `tv` and succeeds, having nothing to write.
        return 0;
    }
    let (seconds, nanoseconds) = read_realtime(true);
    // SAFETY: the caller guarantees a writable struct.
    unsafe {
        ptr::write(
            value,
            TimeVal {
                tv_sec: seconds,
                tv_usec: nanoseconds / 1000,
            },
        );
    }
    0
}

/// Maps a Win32 failure from a clock-setting call onto an errno.
///
/// The refusals are reported as `EPERM`, which is what Linux gives a caller
/// without `CAP_SYS_TIME` and is therefore a condition every caller handles. No
/// other outcome is turned into success.
fn clock_write_error() -> i32 {
    // SAFETY: read immediately after the failed call, on the same thread.
    match unsafe { GetLastError() } {
        ERROR_PRIVILEGE_NOT_HELD | ERROR_ACCESS_DENIED => EPERM,
        ERROR_INVALID_PARAMETER => EINVAL,
        // Anything else is still a refusal to change the clock, and EPERM is the
        // errno that describes it. This is not a claim of success.
        _ => EPERM,
    }
}

/// Steps the wall clock to `seconds` since the epoch.
///
/// The real `SetSystemTime` is called. Without `SE_SYSTEMTIME_NAME` enabled in the
/// process token it fails, and that failure is reported rather than swallowed: a
/// guest that believes it set the clock and finds it unchanged is worse off than
/// one told it lacked permission.
fn set_wall_clock(seconds: i64, nanoseconds: i64) -> Result<(), i32> {
    // The fields are computed here rather than through `SystemTimeToFileTime` for
    // the same reason as everywhere else in this module: our own conversion has no
    // range restrictions to trip over.
    let mut tm = Tm::default();
    fields_from_seconds(seconds, &mut tm);
    let (year, month, _, _, _, _) = fields_of(&tm);
    // `SYSTEMTIME.year` is a `USHORT`, so an instant outside 1601..=30827 has no
    // representation and cannot be requested of the host at all.
    if !(1601..=30827).contains(&year) {
        return Err(EINVAL);
    }
    let time = SystemTime {
        year: year as u16,
        month: (month + 1) as u16,
        day_of_week: tm.tm_wday as u16,
        day: tm.tm_mday as u16,
        hour: tm.tm_hour as u16,
        minute: tm.tm_min as u16,
        second: tm.tm_sec as u16,
        milliseconds: (nanoseconds / 1_000_000) as u16,
    };
    require_clock_write_capability()?;
    // SAFETY: `time` is a fully initialized local of the documented type.
    if unsafe { SetSystemTime(&raw const time) } == 0 {
        return Err(clock_write_error());
    }
    Ok(())
}

/// Host token privileges do not grant capabilities to a Linux process. Check
/// the same effective set returned by capget before touching the host clock.
fn require_clock_write_capability() -> Result<(), i32> {
    const CAP_SYS_TIME: u64 = 1 << 25;
    if crate::userdb::effective_capabilities()? & CAP_SYS_TIME == 0 {
        Err(EPERM)
    } else {
        Ok(())
    }
}

/// `clock_settime`.
///
/// Only `CLOCK_REALTIME` can be set, on Linux as here. The monotonic and CPU
/// clocks have no setter on either platform, and `EINVAL` is what Linux reports for
/// an attempt on one.
///
/// # Safety
///
/// `value` must point at a readable `struct timespec`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_clock_settime(
    clock: c_int,
    value: *const TimeSpec,
) -> c_int {
    if value.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a readable struct.
    let requested = unsafe { ptr::read(value) };
    if !(0..1_000_000_000).contains(&requested.tv_nsec) {
        crate::set_errno(EINVAL);
        return -1;
    }
    if clock != CLOCK_REALTIME {
        crate::set_errno(EINVAL);
        return -1;
    }
    match set_wall_clock(requested.tv_sec, requested.tv_nsec) {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `settimeofday`.
///
/// A non-null `timezone` is accepted and ignored, which is what Linux does: the
/// kernel has ignored the argument since the introduction of zoneinfo, and glibc
/// passes whatever it is given straight through.
///
/// # Safety
///
/// `value` must be null or point at a readable `struct timeval`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_settimeofday(
    value: *const TimeVal,
    _zone: *const c_void,
) -> c_int {
    // A null `tv` with a zone argument was the old way to set the kernel's zone.
    // There is nothing to set and nothing to fail, so it succeeds having done
    // nothing, as Linux does.
    if value.is_null() {
        return 0;
    }
    // SAFETY: the caller guarantees a readable struct.
    let requested = unsafe { ptr::read(value) };
    if !(0..1_000_000).contains(&requested.tv_usec) {
        crate::set_errno(EINVAL);
        return -1;
    }
    match set_wall_clock(requested.tv_sec, requested.tv_usec * 1000) {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `times`, the process's CPU usage in clock ticks.
///
/// Every field is in ticks of `CLOCKS_PER_SECOND`, which is 100 because Linux fixes
/// `USER_HZ` at 100 and `sysconf(_SC_CLK_TCK)` in `userdb.rs` reports 100. A caller
/// divides the result by what `sysconf` told it, so the two must agree; if this
/// scaled to the FILETIME tick instead, every reported CPU time would be out by a
/// factor of 100000.
///
/// The child fields are zero. Windows keeps no aggregated CPU accounting for exited
/// children, and this layer does not accumulate it at `wait` time, so there is no
/// figure to report; zero is what Linux itself shows before any child is reaped.
///
/// The return value is an arbitrary reference point, per POSIX, and is taken from
/// the boot tick so that differences between two calls are meaningful.
///
/// # Safety
///
/// `buffer` must be null or point at a writable `struct tms`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_times(buffer: *mut Tms) -> Time {
    if !buffer.is_null() {
        let mut result = Tms::default();
        let mut creation = FileTime::default();
        let mut exit = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();
        // SAFETY: the pseudo-handle is always valid and all four out-parameters
        // are writable locals.
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &raw mut creation,
                &raw mut exit,
                &raw mut kernel,
                &raw mut user,
            )
        };
        if ok != 0 {
            // FILETIME ticks to clock ticks: 10 MHz down to 100 Hz.
            let per_tick = TICKS_PER_SECOND / CLOCKS_PER_SECOND;
            result.tms_utime = (user.ticks() / per_tick as u64) as Time;
            result.tms_stime = (kernel.ticks() / per_tick as u64) as Time;
        }
        // SAFETY: the caller guarantees a writable struct.
        unsafe { ptr::write(buffer, result) };
    }
    // SAFETY: no preconditions.
    let milliseconds = unsafe { GetTickCount64() } as i64;
    milliseconds / (1000 / CLOCKS_PER_SECOND)
}

/// `adjtimex` mode bits.
pub const ADJ_OFFSET: c_int = 0x0001;
pub const ADJ_FREQUENCY: c_int = 0x0002;
pub const ADJ_MAXERROR: c_int = 0x0004;
pub const ADJ_ESTERROR: c_int = 0x0008;
pub const ADJ_STATUS: c_int = 0x0010;
pub const ADJ_TIMECONST: c_int = 0x0020;
pub const ADJ_TAI: c_int = 0x0080;
pub const ADJ_SETOFFSET: c_int = 0x0100;
pub const ADJ_MICRO: c_int = 0x1000;
pub const ADJ_NANO: c_int = 0x2000;
pub const ADJ_TICK: c_int = 0x4000;
/// `ADJ_OFFSET_SINGLESHOT` and `ADJ_OFFSET_SS_READ` are **composites**, not single
/// bits: they are `0x8001` and `0xa001`, which both contain `ADJ_OFFSET`. They are
/// spelled out from the parts so that masking against them cannot accidentally
/// clear `ADJ_OFFSET` — which is exactly what happened when they were treated as
/// atomic bits, and it made an `ADJ_OFFSET` request look like a read.
pub const ADJ_OFFSET_SINGLESHOT: c_int = ADJ_SINGLESHOT | ADJ_OFFSET;
pub const ADJ_OFFSET_SS_READ: c_int = ADJ_SINGLESHOT | ADJ_NANO | ADJ_OFFSET;
/// The bit that turns `ADJ_OFFSET` into the legacy one-shot `adjtime` request.
const ADJ_SINGLESHOT: c_int = 0x8000;

/// `adjtimex` clock states, which are the call's return value.
pub const TIME_OK: c_int = 0;
pub const TIME_ERROR: c_int = 5;

/// `STA_UNSYNC`: the clock is not being steered by a locked loop.
pub const STA_UNSYNC: c_int = 0x0040;

/// Every mode bit this module recognises.
const KNOWN_MODES: c_int = ADJ_OFFSET
    | ADJ_FREQUENCY
    | ADJ_MAXERROR
    | ADJ_ESTERROR
    | ADJ_STATUS
    | ADJ_TIMECONST
    | ADJ_TAI
    | ADJ_SETOFFSET
    | ADJ_MICRO
    | ADJ_NANO
    | ADJ_TICK
    | ADJ_SINGLESHOT;

/// The mode bits that name something this module can actually carry out.
const ACTED_MODES: c_int = ADJ_FREQUENCY | ADJ_TICK | ADJ_SETOFFSET;

/// `adjtimex`, and by the same code path `ntp_adjtime`.
///
/// **Read mode** (`modes == 0`) reports real figures: `time` from the wall clock,
/// `tick` and `freq` derived from `GetSystemTimeAdjustment`, which is the host's
/// actual per-interrupt clock increment and its actual current slew.
///
/// **`ADJ_FREQUENCY` and `ADJ_TICK` act.** Both name a rate change, which is
/// exactly what `SetSystemTimeAdjustment` does, so the requested rate is converted
/// into an increment and applied. Success means the host accepted it.
///
/// **`ADJ_SETOFFSET` acts**, by reading the clock and stepping it — the same
/// `SetSystemTime` the rest of this module uses, with the same `EPERM` on refusal.
///
/// **Every other write mode is refused with `ENOSYS`, and nothing is applied.**
/// `ADJ_OFFSET`, `ADJ_TIMECONST` and `ADJ_STATUS` steer a phase-locked loop; there
/// is no PLL here and no Windows facility that is one. Accepting those bits and
/// storing them would leave a caller believing it had begun a gradual correction
/// that will never happen, which is materially worse than a refusal it can detect.
/// `ADJ_MAXERROR`, `ADJ_ESTERROR` and `ADJ_TAI` set fields nothing would ever read.
///
/// `status` carries `STA_UNSYNC` and the return is `TIME_ERROR`. That is a
/// statement about the loop, not about the machine's clock accuracy: `STA_UNSYNC`
/// means no synchronization loop holds a lock, and here there is no loop at all.
/// Windows may well be running w32time, but this module cannot attest to it, and
/// claiming a lock it cannot verify would be the fabrication.
///
/// # Safety
///
/// `buffer` must point at a writable `struct timex`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_adjtimex(buffer: *mut Timex) -> c_int {
    if buffer.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a readable and writable struct.
    let request = unsafe { ptr::read(buffer) };
    let modes = request.modes;

    // A mode bit outside the set Linux defines is EINVAL, not a silent ignore.
    if modes & !KNOWN_MODES != 0 {
        crate::set_errno(EINVAL);
        return -1;
    }
    // A recognized bit that cannot be honoured is refused before anything is
    // applied, so a mixed request never takes partial effect. Only the three flag
    // bits are cleared here — `ADJ_MICRO` and `ADJ_NANO` select the unit of the
    // `time` field and `ADJ_SINGLESHOT` marks the legacy `adjtime` form; none of
    // them names a field to change. `ADJ_OFFSET_SS_READ` must not appear in this
    // mask, because it contains `ADJ_OFFSET` and would clear a real request.
    let write_modes = modes & !(ADJ_MICRO | ADJ_NANO | ADJ_SINGLESHOT);
    if write_modes & !ACTED_MODES != 0 {
        crate::set_errno(ENOSYS);
        return -1;
    }

    let mut adjustment = 0u32;
    let mut increment = 0u32;
    let mut disabled = 0i32;
    // SAFETY: three writable locals of the documented types.
    let queried = unsafe {
        GetSystemTimeAdjustment(&raw mut adjustment, &raw mut increment, &raw mut disabled)
    };
    if queried == 0 {
        crate::set_errno(EPERM);
        return -1;
    }

    if modes & ADJ_SETOFFSET != 0 {
        // The offset is a `timeval` normally and a `timespec` under `ADJ_NANO`;
        // both are two 64-bit fields, so only the scale of the second differs.
        let (seconds, fraction) = (request.time.tv_sec, request.time.tv_usec);
        let nanoseconds = if modes & ADJ_NANO != 0 {
            fraction
        } else {
            fraction * 1000
        };
        let (now_seconds, now_nanoseconds) = read_realtime(true);
        let total = (now_seconds + seconds) * 1_000_000_000 + now_nanoseconds + nanoseconds;
        if let Err(error) = set_wall_clock(
            total.div_euclid(1_000_000_000),
            total.rem_euclid(1_000_000_000),
        ) {
            crate::set_errno(error);
            return -1;
        }
    }

    if modes & (ADJ_FREQUENCY | ADJ_TICK) != 0 {
        if let Err(error) = require_clock_write_capability() {
            crate::set_errno(error);
            return -1;
        }
        // `freq` is parts per million scaled by 2^16, so the multiplier is
        // 1 + freq / 65536 / 1e6. It is applied to the nominal increment, which is
        // what `SetSystemTimeAdjustment` expects in 100 ns units.
        let requested = if modes & ADJ_FREQUENCY != 0 {
            let scaled = i64::from(increment) * request.freq;
            i64::from(increment) + scaled / (65_536 * 1_000_000)
        } else {
            // `ADJ_TICK` states microseconds per tick directly, so it converts to
            // the 100 ns unit with no reference to the nominal value.
            request.tick * 10
        };
        // A non-positive increment would stop the clock, which no caller means.
        if requested <= 0 || requested > i64::from(u32::MAX) {
            crate::set_errno(EINVAL);
            return -1;
        }
        // SAFETY: no preconditions beyond the privilege the return value reports.
        if unsafe { SetSystemTimeAdjustment(requested as u32, 0) } == 0 {
            crate::set_errno(clock_write_error());
            return -1;
        }
        // Re-read so the values reported below describe what is now in force.
        // SAFETY: three writable locals.
        unsafe {
            GetSystemTimeAdjustment(&raw mut adjustment, &raw mut increment, &raw mut disabled);
        }
    }

    let (seconds, nanoseconds) = read_realtime(true);
    // The current slew as ppm scaled by 2^16, worked back out of the two
    // increments. This is a real measurement of what the host clock is doing.
    let freq = if increment == 0 {
        0
    } else {
        (i64::from(adjustment) - i64::from(increment)) * 65_536 * 1_000_000 / i64::from(increment)
    };

    let result = Timex {
        modes: 0,
        status: STA_UNSYNC,
        // The host's real per-interrupt increment in microseconds. On Linux this
        // field is 10000 by definition of USER_HZ; here it is what the machine
        // actually does, which is the figure a caller reading `tick` wants.
        tick: i64::from(increment) / 10,
        freq,
        time: TimeVal {
            tv_sec: seconds,
            // `ADJ_NANO` selects the unit of the `time` field on the way out too.
            tv_usec: if modes & ADJ_NANO != 0 {
                nanoseconds
            } else {
                nanoseconds / 1000
            },
        },
        // Deliberately zero rather than invented; see this function's docs.
        ..Timex::default()
    };
    // SAFETY: the caller guarantees a writable struct.
    unsafe { ptr::write(buffer, result) };
    // Consistent with STA_UNSYNC, which is what this state code reports.
    TIME_ERROR
}

// ---------------------------------------------------------------------------
// Broken-down time.
//
// `localtime`, `gmtime`, `ctime` and `asctime` return a pointer to storage the
// caller does not own. glibc uses one process-wide buffer per function, which makes
// two threads calling `localtime` corrupt each other's result.
//
// This module uses **thread-local** buffers instead. The published contract is
// unchanged — the result is valid until the next call on the same thread — but a
// second thread can no longer overwrite it mid-use. That is strictly safer than
// glibc and no caller can tell the difference, because no correct caller relies on
// two threads sharing the buffer. BusyBox `ls` formats timestamps in a loop and
// would be a real corruption risk with a shared buffer.
//
// `UnsafeCell` is used rather than `Cell` because a raw pointer to the storage has
// to outlive the call, which `Cell::get` cannot provide.
// ---------------------------------------------------------------------------

thread_local! {
    /// Backing store for `gmtime`.
    static GMTIME_BUFFER: core::cell::UnsafeCell<Tm> = const {
        core::cell::UnsafeCell::new(Tm {
            tm_sec: 0, tm_min: 0, tm_hour: 0, tm_mday: 1, tm_mon: 0, tm_year: 70,
            tm_wday: 0, tm_yday: 0, tm_isdst: 0, tm_gmtoff: 0, tm_zone: ptr::null(),
        })
    };
    /// Backing store for `localtime`, kept separate because glibc's are separate
    /// and a caller may legitimately hold one across a call to the other.
    static LOCALTIME_BUFFER: core::cell::UnsafeCell<Tm> = const {
        core::cell::UnsafeCell::new(Tm {
            tm_sec: 0, tm_min: 0, tm_hour: 0, tm_mday: 1, tm_mon: 0, tm_year: 70,
            tm_wday: 0, tm_yday: 0, tm_isdst: 0, tm_gmtoff: 0, tm_zone: ptr::null(),
        })
    };
    /// Backing store shared by `asctime` and `ctime`, as glibc's is.
    ///
    /// 64 bytes rather than the 26 the format needs: a year outside 0..=9999
    /// widens the output, and glibc has a real buffer overflow there. The extra
    /// room plus the length check in `format_asctime` closes it.
    static ASCTIME_BUFFER: core::cell::UnsafeCell<[u8; 64]> =
        const { core::cell::UnsafeCell::new([0; 64]) };
}

/// `gmtime_r`: the UTC date for `clock`.
///
/// `tm_gmtoff` is zero and `tm_zone` is `"UTC"`, which is the truth for this
/// conversion rather than a placeholder.
///
/// # Safety
///
/// `clock` must point at a readable `time_t` and `result` at a writable `struct tm`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_gmtime_r(
    clock: *const Time,
    result: *mut Tm,
) -> *mut Tm {
    if clock.is_null() || result.is_null() {
        crate::set_errno(EFAULT);
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees a readable `time_t`.
    let seconds = unsafe { ptr::read(clock) };
    let mut tm = Tm::default();
    fields_from_seconds(seconds, &mut tm);
    tm.tm_isdst = 0;
    tm.tm_gmtoff = 0;
    tm.tm_zone = published_abbrev(ZoneAbbrev::new("UTC"));
    // SAFETY: the caller guarantees a writable struct.
    unsafe { ptr::write(result, tm) };
    result
}

/// `gmtime`, returning a pointer to per-thread storage.
///
/// # Safety
///
/// `clock` must point at a readable `time_t`. The result is valid until the next
/// call to this function on the same thread.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_gmtime(clock: *const Time) -> *mut Tm {
    let buffer = GMTIME_BUFFER.with(core::cell::UnsafeCell::get);
    // SAFETY: `clock` is the caller's to guarantee; `buffer` is this thread's own
    // storage and no other thread can reach it.
    unsafe { kinakaze_abi_gmtime_r(clock, buffer) }
}

/// `localtime_r`: the local date for `clock`.
///
/// The zone comes from `TZ` when set and from Windows otherwise, and `tm_gmtoff`
/// is the real offset in force *at that instant* — not today's offset applied to a
/// timestamp from another season, which is the usual way to be an hour out.
///
/// # Safety
///
/// `clock` must point at a readable `time_t` and `result` at a writable `struct tm`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_localtime_r(
    clock: *const Time,
    result: *mut Tm,
) -> *mut Tm {
    if clock.is_null() || result.is_null() {
        crate::set_errno(EFAULT);
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees a readable `time_t`.
    let seconds = unsafe { ptr::read(clock) };
    let rules = zone_rules();
    zone_globals::publish(rules, seconds);
    let offset = offset_at(rules, seconds);
    let mut tm = Tm::default();
    // Local time is UTC shifted by the offset, and `tm_gmtoff` records the shift so
    // the value remains convertible back.
    fields_from_seconds(seconds + offset.seconds, &mut tm);
    apply_zone(&mut tm, offset);
    // SAFETY: the caller guarantees a writable struct.
    unsafe { ptr::write(result, tm) };
    result
}

/// `localtime`, returning a pointer to per-thread storage.
///
/// # Safety
///
/// `clock` must point at a readable `time_t`. The result is valid until the next
/// call to this function on the same thread.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_localtime(clock: *const Time) -> *mut Tm {
    let buffer = LOCALTIME_BUFFER.with(core::cell::UnsafeCell::get);
    // SAFETY: `clock` is the caller's to guarantee; `buffer` is thread-local.
    unsafe { kinakaze_abi_localtime_r(clock, buffer) }
}

/// `timegm`: the UTC instant a broken-down time names.
///
/// Out-of-range fields are normalised and `tm_wday`, `tm_yday` and the zone fields
/// are written back, matching glibc.
///
/// # Safety
///
/// `tm` must point at a readable and writable `struct tm`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_timegm(tm: *mut Tm) -> Time {
    if tm.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a readable struct.
    let request = unsafe { ptr::read(tm) };
    let (year, month, day, hour, minute, second) = fields_of(&request);
    let seconds = seconds_from_fields(year, month, day, hour, minute, second);

    // Write back the normalised form, which is what makes `timegm` usable as a
    // normaliser in its own right.
    let mut normalised = request;
    fields_from_seconds(seconds, &mut normalised);
    normalised.tm_isdst = 0;
    normalised.tm_gmtoff = 0;
    normalised.tm_zone = published_abbrev(ZoneAbbrev::new("UTC"));
    // SAFETY: the caller guarantees a writable struct.
    unsafe { ptr::write(tm, normalised) };
    seconds
}

/// `mktime`: the UTC instant a broken-down **local** time names.
///
/// Two behaviours callers depend on, both implemented rather than approximated.
///
/// Out-of-range fields are **normalised**: a `tm_mon` of 12 is January of the next
/// year, a `tm_mday` of 0 is the last day of the previous month, and a `tm_hour` of
/// 24 is midnight the next day. BusyBox `date -d` constructs such values on purpose
/// and reads the normalised struct back. This is also why the conversion is written
/// here rather than routed through `SystemTimeToFileTime`, which rejects exactly
/// these inputs.
///
/// `tm_wday` and `tm_yday` are **ignored on input and rewritten on output**, so a
/// caller may leave them uninitialized, as the standard permits.
///
/// # Safety
///
/// `tm` must point at a readable and writable `struct tm`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mktime(tm: *mut Tm) -> Time {
    if tm.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a readable struct.
    let request = unsafe { ptr::read(tm) };
    let (year, month, day, hour, minute, second) = fields_of(&request);
    // The normalised local wall-clock instant, still in local terms.
    let local = seconds_from_fields(year, month, day, hour, minute, second);

    let rules = zone_rules();
    let offset = offset_for_local(rules, local, request.tm_isdst);
    let utc = local - offset.seconds;
    zone_globals::publish(rules, utc);

    // The written-back struct describes the same instant, so the fields come from
    // the UTC value shifted by the offset that was actually chosen. Recomputing
    // from `local` would be equivalent but would not survive the DST-gap case,
    // where the chosen offset moves the wall time.
    let mut normalised = request;
    fields_from_seconds(utc + offset.seconds, &mut normalised);
    apply_zone(&mut normalised, offset);
    // SAFETY: the caller guarantees a writable struct.
    unsafe { ptr::write(tm, normalised) };
    utc
}

/// `difftime`, the seconds between two instants.
///
/// C specifies a `double` return. The conversion is lossless for any timestamp
/// within about 285 million years of the epoch, since a `double` holds 53 bits of
/// integer exactly, so no caller can observe the widening.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_difftime(end: Time, start: Time) -> f64 {
    // Subtracted as integers first so the difference is exact before widening.
    end.wrapping_sub(start) as f64
}

/// Abbreviated weekday names in the C locale, `tm_wday` order.
const WEEKDAY_ABBREV: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
/// Full weekday names in the C locale.
const WEEKDAY_FULL: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
/// Abbreviated month names in the C locale, `tm_mon` order.
const MONTH_ABBREV: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
/// Full month names in the C locale.
const MONTH_FULL: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Indexes a name table with a possibly-out-of-range field.
///
/// A `struct tm` handed in by a guest may hold anything. glibc reads out of bounds
/// for an out-of-range `tm_wday`, which is a real bug; returning `"?"` keeps the
/// output obviously wrong instead of leaking adjacent memory into it.
fn name_at(table: &[&'static str], index: c_int) -> &'static str {
    usize::try_from(index)
        .ok()
        .and_then(|index| table.get(index))
        .copied()
        .unwrap_or("?")
}

/// Renders the 26-byte `asctime` form: `"Thu Jan  1 00:00:00 1970\n"`.
///
/// The day is space-padded and the year is not padded at all, which is what the
/// standard specifies and what callers parse.
fn format_asctime(tm: &Tm) -> String {
    format!(
        "{} {} {:2} {:02}:{:02}:{:02} {}\n",
        name_at(&WEEKDAY_ABBREV, tm.tm_wday),
        name_at(&MONTH_ABBREV, tm.tm_mon),
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        i64::from(tm.tm_year) + 1900,
    )
}

/// Copies `text` and a NUL into `buffer`, reporting whether it fit.
///
/// # Safety
///
/// `buffer` must name at least `capacity` writable bytes.
unsafe fn copy_out(text: &str, buffer: *mut c_char, capacity: usize) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() + 1 > capacity {
        return false;
    }
    // SAFETY: the check above proved the bytes and the terminator fit, and the
    // source is an owned Rust string that cannot overlap the caller's buffer.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast::<u8>(), bytes.len());
        *buffer.add(bytes.len()) = 0;
    }
    true
}

/// The buffer size `asctime_r` and `ctime_r` are documented to require.
const ASCTIME_LENGTH: usize = 26;

/// `asctime_r`.
///
/// The caller's buffer must hold 26 bytes, which is what the interface specifies.
/// A year outside 0..=9999 renders wider than that; glibc overruns the buffer in
/// that case, and this reports `EOVERFLOW` and writes nothing instead.
///
/// # Safety
///
/// `tm` must point at a readable `struct tm` and `buffer` at 26 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_asctime_r(
    tm: *const Tm,
    buffer: *mut c_char,
) -> *mut c_char {
    if tm.is_null() || buffer.is_null() {
        crate::set_errno(EFAULT);
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees a readable struct.
    let text = format_asctime(&unsafe { ptr::read(tm) });
    // SAFETY: the caller guarantees 26 writable bytes.
    if unsafe { !copy_out(&text, buffer, ASCTIME_LENGTH) } {
        // EOVERFLOW, 75 on Linux. Not in the VFS list, so it is named here.
        const EOVERFLOW: i32 = 75;
        crate::set_errno(EOVERFLOW);
        return ptr::null_mut();
    }
    buffer
}

/// `asctime`, returning a pointer to per-thread storage.
///
/// # Safety
///
/// `tm` must point at a readable `struct tm`. The result is valid until the next
/// call to this function or `ctime` on the same thread.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_asctime(tm: *const Tm) -> *mut c_char {
    if tm.is_null() {
        crate::set_errno(EFAULT);
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees a readable struct.
    let text = format_asctime(&unsafe { ptr::read(tm) });
    let buffer = ASCTIME_BUFFER
        .with(core::cell::UnsafeCell::get)
        .cast::<c_char>();
    // The 64-byte buffer accommodates the wide-year case the `_r` form must
    // refuse, so this cannot fail for any representable year.
    // SAFETY: the buffer is this thread's own 64 bytes.
    if unsafe { !copy_out(&text, buffer, 64) } {
        return ptr::null_mut();
    }
    buffer
}

/// `ctime_r`: `asctime_r(localtime_r(clock))`, per the standard.
///
/// # Safety
///
/// `clock` must point at a readable `time_t` and `buffer` at 26 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ctime_r(
    clock: *const Time,
    buffer: *mut c_char,
) -> *mut c_char {
    let mut tm = Tm::default();
    // SAFETY: `clock` is the caller's to guarantee; `tm` is a writable local.
    if unsafe { kinakaze_abi_localtime_r(clock, &raw mut tm) }.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `tm` is initialized and `buffer` is the caller's to guarantee.
    unsafe { kinakaze_abi_asctime_r(&raw const tm, buffer) }
}

/// `ctime`, the local date as text, using per-thread storage.
///
/// # Safety
///
/// `clock` must point at a readable `time_t`. The result is valid until the next
/// call to this function or `asctime` on the same thread.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ctime(clock: *const Time) -> *mut c_char {
    let mut tm = Tm::default();
    // SAFETY: `clock` is the caller's to guarantee; `tm` is a writable local.
    if unsafe { kinakaze_abi_localtime_r(clock, &raw mut tm) }.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `tm` is fully initialized.
    unsafe { kinakaze_abi_asctime(&raw const tm) }
}

/// `tzset`, which re-reads the zone configuration.
///
/// `TZ` is honoured: `UTC`, `GMT`, `STDoffset`, `STDoffsetDST` and the full
/// `STDoffsetDST,start,end` form with `Mm.w.d`, `Jn` and `n` transitions. A value
/// naming a zoneinfo file — anything starting with `:` or containing `/`, so
/// `Europe/Berlin` included — cannot be resolved on a host with no IANA database
/// and falls back to the Windows zone. [`parse_tz`] documents that in full.
///
/// Publish both standard/daylight names, the standard seconds west of UTC,
/// and whether this zone has daylight rules. Conversions use the same publisher.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_tzset() {
    let (seconds, _) = read_realtime(false);
    zone_globals::publish(zone_rules(), seconds);
}

// ---------------------------------------------------------------------------
// ISO 8601 week numbering.
//
// `%V` is not `%U` or `%W` with a different starting weekday, and aliasing it to
// either is the classic way to get this wrong. ISO weeks start on Monday, week 1 is
// the week containing the first Thursday of the year, and the first days of January
// therefore often belong to week 52 or 53 of the *previous* year — with `%G`
// reporting that previous year while `%Y` still reports this one. 2021-01-01 is
// ISO 2020-W53, which is the case the tests pin.
// ---------------------------------------------------------------------------

/// The weekday of December 31st of `year`, as an ISO index where Monday is 1.
///
/// Used only to decide how many ISO weeks a year has.
fn year_end_weekday(year: i64) -> i64 {
    (year + year.div_euclid(4) - year.div_euclid(100) + year.div_euclid(400)).rem_euclid(7)
}

/// The number of ISO weeks in `year`: 52, or 53 for a long year.
///
/// A year is long when it ends on a Thursday, or when the year before it ended on a
/// Wednesday and it is itself a leap year — the standard's rule stated directly.
fn iso_weeks_in_year(year: i64) -> i64 {
    if year_end_weekday(year) == 4 || year_end_weekday(year - 1) == 3 {
        53
    } else {
        52
    }
}

/// The ISO year and week for a date, given the calendar year, zero-based day of
/// year and `tm_wday`.
fn iso_year_week(year: i64, yday: i64, wday: i64) -> (i64, i64) {
    // ISO counts Monday as 1 and Sunday as 7, where `tm_wday` has Sunday as 0.
    let iso_weekday = if wday == 0 { 7 } else { wday };
    let ordinal = yday + 1;
    let week = (ordinal - iso_weekday + 10).div_euclid(7);
    if week < 1 {
        // Early January falling in the last week of the previous ISO year.
        return (year - 1, iso_weeks_in_year(year - 1));
    }
    if week > iso_weeks_in_year(year) {
        // Late December falling in week 1 of the next ISO year.
        return (year + 1, 1);
    }
    (year, week)
}

/// How a field is padded, before the `%` flags are applied.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Pad {
    /// Whatever the conversion specifies.
    Default,
    Zero,
    Space,
    /// The `-` flag: no padding at all.
    None,
}

/// One parsed conversion's flags and width.
struct Spec {
    pad: Pad,
    /// `^`: force upper case.
    upper: bool,
    /// `#`: swap the case the conversion would normally use.
    swap: bool,
    /// An explicit field width, or 0 for the conversion's default.
    width: usize,
}

/// Appends `value` as a decimal number, padded per `spec`.
fn emit_number(out: &mut Vec<u8>, spec: &Spec, value: i64, default_width: usize, default_pad: u8) {
    let pad = match spec.pad {
        Pad::Default => default_pad,
        Pad::Zero => b'0',
        Pad::Space => b' ',
        Pad::None => 0,
    };
    let width = if spec.width > 0 {
        spec.width
    } else {
        default_width
    };
    let digits = value.unsigned_abs().to_string();
    let sign: &[u8] = if value < 0 { b"-" } else { b"" };

    if pad == 0 {
        out.extend_from_slice(sign);
        out.extend_from_slice(digits.as_bytes());
        return;
    }
    let filled = digits.len() + sign.len();
    let padding = width.saturating_sub(filled);
    if pad == b'0' {
        // A zero-padded negative number keeps its sign in front of the zeros.
        out.extend_from_slice(sign);
        out.extend(core::iter::repeat_n(b'0', padding));
        out.extend_from_slice(digits.as_bytes());
    } else {
        out.extend(core::iter::repeat_n(pad, padding));
        out.extend_from_slice(sign);
        out.extend_from_slice(digits.as_bytes());
    }
}

/// Appends `text`, applying the case flags and any explicit width.
fn emit_text(out: &mut Vec<u8>, spec: &Spec, text: &str) {
    let padding = spec.width.saturating_sub(text.len());
    let pad = match spec.pad {
        Pad::Zero => b'0',
        Pad::None => 0,
        // Text fields pad with spaces by default.
        _ => b' ',
    };
    if pad != 0 {
        out.extend(core::iter::repeat_n(pad, padding));
    }
    for byte in text.bytes() {
        let byte = if spec.upper {
            byte.to_ascii_uppercase()
        } else if spec.swap {
            // `#` inverts the case the conversion produces, which for the name and
            // AM/PM conversions means lowering the leading capital.
            if byte.is_ascii_uppercase() {
                byte.to_ascii_lowercase()
            } else {
                byte.to_ascii_uppercase()
            }
        } else {
            byte
        };
        out.push(byte);
    }
}

/// Appends the numeric zone offset, `+hhmm`.
fn emit_zone_offset(out: &mut Vec<u8>, offset: i64) {
    let sign = if offset < 0 { b'-' } else { b'+' };
    let magnitude = offset.abs();
    out.push(sign);
    let text = format!("{:02}{:02}", magnitude / 3600, (magnitude / 60) % 60);
    out.extend_from_slice(text.as_bytes());
}

/// The zone abbreviation for `%Z`.
///
/// `tm_zone` is used when the caller left the one this module wrote in place. A
/// `struct tm` the guest built itself may leave it null, in which case the numeric
/// form derived from `tm_gmtoff` is the honest answer — it describes the offset the
/// struct actually carries rather than guessing at a name for it.
fn zone_name_of(tm: &Tm) -> ZoneAbbrev {
    if tm.tm_zone.is_null() {
        return ZoneAbbrev::numeric(tm.tm_gmtoff);
    }
    // SAFETY: `tm_zone` is either a pointer this module interned, which is a live
    // NUL-terminated string for the life of the process, or one the caller
    // supplied, which the C interface requires to be a valid string. glibc
    // dereferences it on the same terms.
    match unsafe { CStr::from_ptr(tm.tm_zone) }.to_str() {
        Ok(name) => ZoneAbbrev::new(name),
        Err(_) => ZoneAbbrev::numeric(tm.tm_gmtoff),
    }
}

/// The hour in 12-hour form, where both 0 and 12 render as 12.
fn hour_twelve(hour: c_int) -> i64 {
    let hour = i64::from(hour).rem_euclid(24);
    match hour % 12 {
        0 => 12,
        other => other,
    }
}

/// Renders `format` for `tm`, appending to `out`.
///
/// `depth` bounds the recursion the composite conversions use. `%c`, `%r` and the
/// rest expand to formats containing only simple conversions, so one level is
/// always enough; the guard exists so a future edit cannot turn a typo into
/// unbounded recursion inside a libc call.
fn render(out: &mut Vec<u8>, format: &[u8], tm: &Tm, depth: u32) {
    /// Expands a composite conversion by rendering its equivalent format.
    fn composite(out: &mut Vec<u8>, pattern: &[u8], tm: &Tm, depth: u32) {
        if depth == 0 {
            return;
        }
        render(out, pattern, tm, depth - 1);
    }

    let year = i64::from(tm.tm_year) + 1900;
    let yday = i64::from(tm.tm_yday);
    let wday = i64::from(tm.tm_wday);

    let mut cursor = 0;
    while cursor < format.len() {
        if format[cursor] != b'%' {
            out.push(format[cursor]);
            cursor += 1;
            continue;
        }
        cursor += 1;
        // A trailing '%' is emitted literally, which is what glibc does.
        if cursor >= format.len() {
            out.push(b'%');
            break;
        }

        // Flags, which may appear in any order and any number.
        let mut spec = Spec {
            pad: Pad::Default,
            upper: false,
            swap: false,
            width: 0,
        };
        while cursor < format.len() {
            match format[cursor] {
                b'-' => spec.pad = Pad::None,
                b'_' => spec.pad = Pad::Space,
                b'0' => spec.pad = Pad::Zero,
                b'^' => spec.upper = true,
                b'#' => spec.swap = true,
                _ => break,
            }
            cursor += 1;
        }
        // An optional decimal field width.
        while cursor < format.len() && format[cursor].is_ascii_digit() {
            spec.width = spec.width * 10 + usize::from(format[cursor] - b'0');
            // A width beyond any sane output is clamped rather than allowed to
            // overflow into an allocation the size of the machine.
            spec.width = spec.width.min(1024);
            cursor += 1;
        }
        // The `E` and `O` modifiers select a locale's alternative era or numeric
        // symbols. The C locale defines no alternatives, so glibc ignores them
        // there and so does this — the modifier is consumed and the base
        // conversion applied.
        if cursor < format.len() && (format[cursor] == b'E' || format[cursor] == b'O') {
            cursor += 1;
        }
        if cursor >= format.len() {
            break;
        }
        let conversion = format[cursor];
        cursor += 1;

        match conversion {
            b'a' => emit_text(out, &spec, name_at(&WEEKDAY_ABBREV, tm.tm_wday)),
            b'A' => emit_text(out, &spec, name_at(&WEEKDAY_FULL, tm.tm_wday)),
            b'b' | b'h' => emit_text(out, &spec, name_at(&MONTH_ABBREV, tm.tm_mon)),
            b'B' => emit_text(out, &spec, name_at(&MONTH_FULL, tm.tm_mon)),
            // The C locale's date-and-time form.
            b'c' => composite(out, b"%a %b %e %H:%M:%S %Y", tm, depth),
            // Floor division, so the century of a year before 1 is negative rather
            // than truncated toward zero.
            b'C' => emit_number(out, &spec, year.div_euclid(100), 2, b'0'),
            b'd' => emit_number(out, &spec, i64::from(tm.tm_mday), 2, b'0'),
            b'D' => composite(out, b"%m/%d/%y", tm, depth),
            // Space-padded day, the one `%d` variant `ls -l` output depends on.
            b'e' => emit_number(out, &spec, i64::from(tm.tm_mday), 2, b' '),
            b'F' => composite(out, b"%Y-%m-%d", tm, depth),
            b'g' => {
                let (iso_year, _) = iso_year_week(year, yday, wday);
                emit_number(out, &spec, iso_year.rem_euclid(100), 2, b'0');
            }
            b'G' => {
                let (iso_year, _) = iso_year_week(year, yday, wday);
                emit_number(out, &spec, iso_year, 4, b'0');
            }
            b'H' => emit_number(out, &spec, i64::from(tm.tm_hour), 2, b'0'),
            b'I' => emit_number(out, &spec, hour_twelve(tm.tm_hour), 2, b'0'),
            // `%j` is one-based where `tm_yday` is zero-based.
            b'j' => emit_number(out, &spec, yday + 1, 3, b'0'),
            b'k' => emit_number(out, &spec, i64::from(tm.tm_hour), 2, b' '),
            b'l' => emit_number(out, &spec, hour_twelve(tm.tm_hour), 2, b' '),
            b'm' => emit_number(out, &spec, i64::from(tm.tm_mon) + 1, 2, b'0'),
            b'M' => emit_number(out, &spec, i64::from(tm.tm_min), 2, b'0'),
            b'n' => out.push(b'\n'),
            b'p' => emit_text(out, &spec, if tm.tm_hour % 24 < 12 { "AM" } else { "PM" }),
            // The GNU lower-case form. `^` still upper-cases it, as in glibc.
            b'P' => {
                let text = if tm.tm_hour % 24 < 12 { "am" } else { "pm" };
                if spec.upper {
                    emit_text(out, &spec, &text.to_ascii_uppercase());
                } else {
                    // `#` would swap this to upper case, which for `%P` is the
                    // opposite of the intent, so the flag is not applied.
                    let plain = Spec {
                        swap: false,
                        ..spec
                    };
                    emit_text(out, &plain, text);
                }
            }
            b'r' => composite(out, b"%I:%M:%S %p", tm, depth),
            b'R' => composite(out, b"%H:%M", tm, depth),
            b's' => {
                // Seconds since the epoch for the instant these fields name. The
                // fields are local, so the offset the struct carries is removed to
                // reach UTC — the same relationship `mktime` maintains.
                let (year, month, day, hour, minute, second) = fields_of(tm);
                let local = seconds_from_fields(year, month, day, hour, minute, second);
                emit_number(out, &spec, local - tm.tm_gmtoff, 1, 0);
            }
            b'S' => emit_number(out, &spec, i64::from(tm.tm_sec), 2, b'0'),
            b't' => out.push(b'\t'),
            b'T' => composite(out, b"%H:%M:%S", tm, depth),
            // Monday is 1 and Sunday is 7, unlike `%w`.
            b'u' => emit_number(out, &spec, if wday == 0 { 7 } else { wday }, 1, b'0'),
            // Weeks counted from the first Sunday; days before it are week 0.
            b'U' => emit_number(out, &spec, (yday + 7 - wday) / 7, 2, b'0'),
            b'V' => {
                let (_, week) = iso_year_week(year, yday, wday);
                emit_number(out, &spec, week, 2, b'0');
            }
            b'w' => emit_number(out, &spec, wday, 1, b'0'),
            // Weeks counted from the first Monday.
            b'W' => emit_number(out, &spec, (yday + 7 - (wday + 6) % 7) / 7, 2, b'0'),
            b'x' => composite(out, b"%m/%d/%y", tm, depth),
            b'X' => composite(out, b"%H:%M:%S", tm, depth),
            b'y' => emit_number(out, &spec, year.rem_euclid(100), 2, b'0'),
            b'Y' => emit_number(out, &spec, year, 4, b'0'),
            b'z' => emit_zone_offset(out, tm.tm_gmtoff),
            b'Z' => emit_text(out, &spec, zone_name_of(tm).as_str()),
            b'%' => out.push(b'%'),
            // An unrecognized conversion is emitted verbatim, including its '%'.
            // That is glibc's behaviour and it makes a typo visible in the output
            // rather than silently dropping part of the format.
            other => {
                out.push(b'%');
                out.push(other);
            }
        }
    }
}

/// `strftime`.
///
/// Returns the bytes written excluding the terminator, or **0 when the result does
/// not fit**, in which case the buffer contents are unspecified. Note that 0 is
/// also what an empty format legitimately produces; that ambiguity is in the
/// standard and callers handle it by never passing an empty format.
///
/// Supported: `%a %A %b %B %c %C %d %D %e %F %g %G %h %H %I %j %k %l %m %M %n %p
/// %P %r %R %s %S %t %T %u %U %V %w %W %x %X %y %Y %z %Z %%`, with the `-`, `_`,
/// `0`, `^` and `#` flags, explicit field widths, and the `E`/`O` modifiers
/// accepted and ignored as they are in the C locale.
///
/// Only the C locale exists here. `%c`, `%x` and `%X` therefore use the C locale's
/// forms, which is what a guest running without a locale gets from glibc too, and
/// `%Z` reports what the module header describes.
///
/// # Safety
///
/// `buffer` must name `max` writable bytes, `format` must be null-terminated and
/// `tm` must point at a readable `struct tm`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strftime(
    buffer: *mut c_char,
    max: usize,
    format: *const c_char,
    tm: *const Tm,
) -> usize {
    if buffer.is_null() || format.is_null() || tm.is_null() || max == 0 {
        return 0;
    }
    // SAFETY: the caller guarantees a null-terminated format string.
    let pattern = unsafe { CStr::from_ptr(format) }.to_bytes();
    // SAFETY: the caller guarantees a readable struct.
    let tm = unsafe { ptr::read(tm) };

    let mut rendered = Vec::new();
    render(&mut rendered, pattern, &tm, 2);

    // The terminator has to fit as well, so a result of exactly `max` bytes is
    // still a failure. glibc returns 0 here without a partial write being useful.
    if rendered.len() + 1 > max {
        return 0;
    }
    // SAFETY: the bound above proved the bytes and the terminator fit.
    unsafe {
        ptr::copy_nonoverlapping(rendered.as_ptr(), buffer.cast::<u8>(), rendered.len());
        *buffer.add(rendered.len()) = 0;
    }
    rendered.len()
}

// ---------------------------------------------------------------------------
// strptime.
//
// The parse accumulates into a scratch record rather than writing `tm` field by
// field, because several conversions interact and the order they appear in the
// format is the caller's choice. `%I` and `%p` are the clear case: neither alone
// determines the hour, and `%p` may come first. The 12-hour value and the meridiem
// are therefore held separately and combined once the whole format is consumed.
//
// glibc leaves fields it did not parse untouched, so the caller's `struct tm` is
// read in as the starting point rather than zeroed.
// ---------------------------------------------------------------------------

/// Fields gathered during one `strptime` call.
#[derive(Default)]
struct Parsed {
    year: Option<i64>,
    /// A two-digit year, resolved by the POSIX century rule once parsing ends.
    short_year: Option<i64>,
    century: Option<i64>,
    month: Option<i64>,
    day: Option<i64>,
    hour: Option<i64>,
    /// A 12-hour reading from `%I` or `%l`, meaningless without `%p`.
    hour_twelve: Option<i64>,
    /// True for PM, from `%p` or `%P`.
    afternoon: Option<bool>,
    minute: Option<i64>,
    second: Option<i64>,
    weekday: Option<i64>,
    yday: Option<i64>,
    /// An absolute instant from `%s`, which supersedes everything else.
    epoch: Option<i64>,
    /// An offset from `%z`.
    gmtoff: Option<i64>,
}

/// Skips leading whitespace, returning the remaining input.
fn skip_whitespace(input: &[u8]) -> &[u8] {
    let end = input
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(input.len());
    &input[end..]
}

/// Parses up to `digits` decimal digits, optionally signed.
///
/// Leading whitespace is skipped, which is what lets a `%d` parse the space-padded
/// output of `%e`. Returns `None` when no digit is present, which is a parse
/// failure for every numeric conversion.
fn take_number(input: &[u8], digits: usize, signed: bool) -> Option<(i64, &[u8])> {
    let input = skip_whitespace(input);
    let (negate, rest) = match input.first() {
        Some(b'-') if signed => (true, &input[1..]),
        Some(b'+') if signed => (false, &input[1..]),
        _ => (false, input),
    };
    let mut value: i64 = 0;
    let mut count = 0;
    while count < digits && count < rest.len() && rest[count].is_ascii_digit() {
        value = value * 10 + i64::from(rest[count] - b'0');
        count += 1;
    }
    if count == 0 {
        return None;
    }
    Some((if negate { -value } else { value }, &rest[count..]))
}

/// Matches the longest name in `table` that prefixes `input`, case-insensitively.
///
/// Returns the index and the remaining input. The longest match matters: `"June"`
/// must not be accepted as `"Jun"` with a stray `"e"` left over when the full name
/// is present, because the caller's next literal would then fail to match.
fn take_name(input: &[u8], tables: &[&[&'static str]]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for table in tables {
        for (index, name) in table.iter().enumerate() {
            let bytes = name.as_bytes();
            if input.len() < bytes.len() {
                continue;
            }
            if !input[..bytes.len()].eq_ignore_ascii_case(bytes) {
                continue;
            }
            if best.is_none_or(|(_, length)| bytes.len() > length) {
                best = Some((index, bytes.len()));
            }
        }
    }
    best
}

/// Applies everything gathered to `tm`, deriving the fields that follow.
///
/// `tm_wday` and `tm_yday` are recomputed whenever the full date is known, which is
/// what glibc does and what a caller passing the result to `strftime` "%A" relies
/// on. They are otherwise left as the caller had them, or as `%j`/`%a` set them.
fn apply_parsed(parsed: &Parsed, tm: &mut Tm) {
    // `%s` names an absolute instant, so it determines every field on its own.
    if let Some(epoch) = parsed.epoch {
        fields_from_seconds(epoch, tm);
        tm.tm_isdst = 0;
        tm.tm_gmtoff = 0;
        tm.tm_zone = published_abbrev(ZoneAbbrev::new("UTC"));
        return;
    }

    // The century rule POSIX specifies: a two-digit year of 69..=99 is 1900s and
    // 0..=68 is 2000s. An explicit `%C` overrides it.
    let year = match (parsed.year, parsed.century, parsed.short_year) {
        (Some(year), _, _) => Some(year),
        (None, Some(century), Some(short)) => Some(century * 100 + short),
        (None, Some(century), None) => Some(century * 100),
        (None, None, Some(short)) if short >= 69 => Some(1900 + short),
        (None, None, Some(short)) => Some(2000 + short),
        (None, None, None) => None,
    };
    if let Some(year) = year {
        tm.tm_year = (year - 1900) as c_int;
    }
    if let Some(month) = parsed.month {
        tm.tm_mon = (month - 1) as c_int;
    }
    if let Some(day) = parsed.day {
        tm.tm_mday = day as c_int;
    }
    if let Some(minute) = parsed.minute {
        tm.tm_min = minute as c_int;
    }
    if let Some(second) = parsed.second {
        tm.tm_sec = second as c_int;
    }
    if let Some(weekday) = parsed.weekday {
        tm.tm_wday = weekday as c_int;
    }
    if let Some(yday) = parsed.yday {
        tm.tm_yday = yday as c_int;
    }
    if let Some(offset) = parsed.gmtoff {
        tm.tm_gmtoff = offset;
    }

    // The hour: a 24-hour reading wins, otherwise the 12-hour reading combines
    // with the meridiem. 12 AM is hour 0 and 12 PM is hour 12.
    match (parsed.hour, parsed.hour_twelve, parsed.afternoon) {
        (Some(hour), _, _) => tm.tm_hour = hour as c_int,
        (None, Some(twelve), Some(afternoon)) => {
            let base = if twelve == 12 { 0 } else { twelve };
            tm.tm_hour = (base + if afternoon { 12 } else { 0 }) as c_int;
        }
        (None, Some(twelve), None) => tm.tm_hour = twelve as c_int,
        // `%p` with no hour conversion adjusts nothing, matching glibc.
        (None, None, _) => {}
    }

    // With a complete date, the weekday and day of year follow from it and are
    // recomputed even if `%a` or `%j` set them, because the date is authoritative.
    if year.is_some() && parsed.month.is_some() && parsed.day.is_some() {
        let full_year = i64::from(tm.tm_year) + 1900;
        let days = days_from_civil(full_year, i64::from(tm.tm_mon) + 1, i64::from(tm.tm_mday));
        tm.tm_wday = ((days + EPOCH_WEEKDAY).rem_euclid(7)) as c_int;
        tm.tm_yday = (days - days_from_civil(full_year, 1, 1)) as c_int;
    }
}

/// Consumes `format` against `input`, gathering fields. Returns the unparsed tail.
fn scan<'a>(mut format: &[u8], mut input: &'a [u8], parsed: &mut Parsed) -> Option<&'a [u8]> {
    while let Some((&byte, rest)) = format.split_first() {
        format = rest;
        // Whitespace in the format matches any amount of whitespace, including
        // none, which is what lets one format accept both "Jan 1" and "Jan  1".
        if byte.is_ascii_whitespace() {
            input = skip_whitespace(input);
            continue;
        }
        if byte != b'%' {
            // A literal must match exactly.
            let (&first, rest) = input.split_first()?;
            if first != byte {
                return None;
            }
            input = rest;
            continue;
        }

        // Flags and width are accepted and ignored on input: a width would only
        // narrow what is already a maximum, and glibc ignores them here too.
        while let Some(&next) = format.first() {
            if matches!(next, b'-' | b'_' | b'0' | b'^' | b'#') {
                format = &format[1..];
            } else {
                break;
            }
        }
        while format.first().is_some_and(u8::is_ascii_digit) {
            format = &format[1..];
        }
        // `E` and `O` name locale alternatives the C locale does not have.
        if matches!(format.first(), Some(b'E' | b'O')) {
            format = &format[1..];
        }
        let (&conversion, rest) = format.split_first()?;
        format = rest;

        match conversion {
            b'a' | b'A' => {
                let (index, length) =
                    take_name(skip_whitespace(input), &[&WEEKDAY_FULL, &WEEKDAY_ABBREV])?;
                input = &skip_whitespace(input)[length..];
                parsed.weekday = Some(index as i64);
            }
            b'b' | b'B' | b'h' => {
                let (index, length) =
                    take_name(skip_whitespace(input), &[&MONTH_FULL, &MONTH_ABBREV])?;
                input = &skip_whitespace(input)[length..];
                parsed.month = Some(index as i64 + 1);
            }
            b'c' => input = scan(b"%a %b %e %H:%M:%S %Y", input, parsed)?,
            b'C' => {
                let (value, rest) = take_number(input, 2, false)?;
                parsed.century = Some(value);
                input = rest;
            }
            b'd' | b'e' => {
                let (value, rest) = take_number(input, 2, false)?;
                if !(1..=31).contains(&value) {
                    return None;
                }
                parsed.day = Some(value);
                input = rest;
            }
            b'D' | b'x' => input = scan(b"%m/%d/%y", input, parsed)?,
            b'F' => input = scan(b"%Y-%m-%d", input, parsed)?,
            b'H' | b'k' => {
                let (value, rest) = take_number(input, 2, false)?;
                if !(0..=23).contains(&value) {
                    return None;
                }
                parsed.hour = Some(value);
                input = rest;
            }
            b'I' | b'l' => {
                let (value, rest) = take_number(input, 2, false)?;
                if !(1..=12).contains(&value) {
                    return None;
                }
                parsed.hour_twelve = Some(value);
                input = rest;
            }
            b'j' => {
                let (value, rest) = take_number(input, 3, false)?;
                if !(1..=366).contains(&value) {
                    return None;
                }
                // `%j` is one-based and `tm_yday` is zero-based.
                parsed.yday = Some(value - 1);
                input = rest;
            }
            b'm' => {
                let (value, rest) = take_number(input, 2, false)?;
                if !(1..=12).contains(&value) {
                    return None;
                }
                parsed.month = Some(value);
                input = rest;
            }
            b'M' => {
                let (value, rest) = take_number(input, 2, false)?;
                if !(0..=59).contains(&value) {
                    return None;
                }
                parsed.minute = Some(value);
                input = rest;
            }
            // In the format, both match any run of whitespace.
            b'n' | b't' => input = skip_whitespace(input),
            b'p' | b'P' => {
                let candidate = skip_whitespace(input);
                if candidate.len() < 2 {
                    return None;
                }
                let afternoon = if candidate[..2].eq_ignore_ascii_case(b"AM") {
                    false
                } else if candidate[..2].eq_ignore_ascii_case(b"PM") {
                    true
                } else {
                    return None;
                };
                parsed.afternoon = Some(afternoon);
                input = &candidate[2..];
            }
            b'r' => input = scan(b"%I:%M:%S %p", input, parsed)?,
            b'R' => input = scan(b"%H:%M", input, parsed)?,
            b's' => {
                // Signed: a pre-epoch instant is a legitimate value here.
                let (value, rest) = take_number(input, 20, true)?;
                parsed.epoch = Some(value);
                input = rest;
            }
            b'S' => {
                let (value, rest) = take_number(input, 2, false)?;
                // 60 is accepted for a leap second, as Linux does.
                if !(0..=60).contains(&value) {
                    return None;
                }
                parsed.second = Some(value);
                input = rest;
            }
            b'T' | b'X' => input = scan(b"%H:%M:%S", input, parsed)?,
            b'u' => {
                let (value, rest) = take_number(input, 1, false)?;
                if !(1..=7).contains(&value) {
                    return None;
                }
                // ISO numbering has Monday as 1 and Sunday as 7; `tm_wday` has
                // Sunday as 0.
                parsed.weekday = Some(if value == 7 { 0 } else { value });
                input = rest;
            }
            b'w' => {
                let (value, rest) = take_number(input, 1, false)?;
                if !(0..=6).contains(&value) {
                    return None;
                }
                parsed.weekday = Some(value);
                input = rest;
            }
            // The week-number conversions are consumed and discarded. A week number
            // constrains the date only in combination with a weekday and a year,
            // and glibc does not reconstruct a date from them either — it accepts
            // and ignores them, which is what happens here.
            b'U' | b'V' | b'W' => {
                let (_, rest) = take_number(input, 2, false)?;
                input = rest;
            }
            b'g' => {
                let (_, rest) = take_number(input, 2, false)?;
                input = rest;
            }
            b'G' => {
                let (_, rest) = take_number(input, 4, true)?;
                input = rest;
            }
            b'y' => {
                let (value, rest) = take_number(input, 2, false)?;
                parsed.short_year = Some(value);
                input = rest;
            }
            b'Y' => {
                let (value, rest) = take_number(input, 4, true)?;
                parsed.year = Some(value);
                input = rest;
            }
            b'z' => {
                let candidate = skip_whitespace(input);
                // `Z` alone means UTC, which is what ISO 8601 writes.
                if let Some((&b'Z', rest)) = candidate.split_first() {
                    parsed.gmtoff = Some(0);
                    input = rest;
                    continue;
                }
                let (&sign, rest) = candidate.split_first()?;
                let negate = match sign {
                    b'-' => true,
                    b'+' => false,
                    _ => return None,
                };
                let (hours, rest) = take_number(rest, 2, false)?;
                // Both `+0930` and `+09:30` occur in the wild.
                let rest = rest.strip_prefix(b":").unwrap_or(rest);
                let (minutes, rest) = match take_number(rest, 2, false) {
                    Some((minutes, rest)) => (minutes, rest),
                    // `+09` with no minutes is valid.
                    None => (0, rest),
                };
                let total = hours * 3600 + minutes * 60;
                parsed.gmtoff = Some(if negate { -total } else { total });
                input = rest;
            }
            b'Z' => {
                // A zone name cannot be resolved to rules without the IANA
                // database, so the token is consumed and discarded — the same thing
                // glibc does for a name it does not recognize. Consuming it is what
                // matters, so the rest of the format still lines up.
                let candidate = skip_whitespace(input);
                let end = candidate
                    .iter()
                    .position(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'+' | b'-'))
                    .unwrap_or(candidate.len());
                if end == 0 {
                    return None;
                }
                input = &candidate[end..];
            }
            b'%' => {
                let (&first, rest) = input.split_first()?;
                if first != b'%' {
                    return None;
                }
                input = rest;
            }
            // An unknown conversion cannot be matched against anything, so the
            // parse stops rather than silently skipping input.
            _ => return None,
        }
    }
    Some(input)
}

/// `strptime`.
///
/// Returns a pointer to the first character of `input` not consumed, or null when
/// the input did not match. Fields the format did not mention are left as the
/// caller had them, which is glibc's behaviour and what makes the incremental
/// two-call idiom work.
///
/// Supported: `%a %A %b %B %c %C %d %D %e %F %g %G %h %H %I %j %k %l %m %M %n %p
/// %P %r %R %s %S %t %T %u %w %y %Y %z %Z %%`, plus `%x` and `%X` in their C-locale
/// forms. Flags, widths and the `E`/`O` modifiers are accepted and ignored.
///
/// `%U`, `%V`, `%W` and `%G`/`%g` are **parsed and discarded**: a week number does
/// not determine a date without a weekday and year alongside it, and glibc does not
/// reconstruct one from them either. `%Z` is likewise consumed without effect,
/// because resolving a zone abbreviation needs the IANA database this host lacks.
/// Both are documented divergences rather than silent no-ops — the token is still
/// consumed, so the rest of the format continues to line up.
///
/// # Safety
///
/// `input` and `format` must be null-terminated and `tm` must point at a readable
/// and writable `struct tm`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strptime(
    input: *const c_char,
    format: *const c_char,
    tm: *mut Tm,
) -> *mut c_char {
    if input.is_null() || format.is_null() || tm.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees null-terminated strings.
    let (text, pattern) = unsafe {
        (
            CStr::from_ptr(input).to_bytes(),
            CStr::from_ptr(format).to_bytes(),
        )
    };

    let mut parsed = Parsed::default();
    let Some(remaining) = scan(pattern, text, &mut parsed) else {
        return ptr::null_mut();
    };
    // SAFETY: the caller guarantees a readable struct; unparsed fields keep the
    // values they arrived with.
    let mut result = unsafe { ptr::read(tm) };
    apply_parsed(&parsed, &mut result);
    // SAFETY: the caller guarantees a writable struct.
    unsafe { ptr::write(tm, result) };

    // The returned pointer addresses the tail of the caller's own buffer, which is
    // the documented contract: the offset is measured rather than the pointer being
    // rebuilt, so it stays inside the original allocation.
    let consumed = text.len() - remaining.len();
    // SAFETY: `consumed` is at most `text.len()`, so this stays within the string
    // including its terminator.
    unsafe { input.add(consumed).cast_mut() }
}

// ---------------------------------------------------------------------------
// Sleeping.
// ---------------------------------------------------------------------------

/// How long a sleep waits before checking for a pending signal.
///
/// Signals are delivered by a thread noticing that something is pending rather
/// than by an asynchronous interrupt, so the granularity of that check bounds how
/// promptly a sleep can be broken. One millisecond matches the interval
/// `sigextra.rs` chose for the same reason and is below the resolution of anything
/// a caller can observe through `nanosleep`'s remaining-time output.
const POLL_INTERVAL: Duration = Duration::from_millis(1);

/// The monotonic clock in nanoseconds, which is the unit the timers work in.
fn monotonic_nanoseconds() -> i64 {
    let (seconds, nanoseconds) = read_monotonic();
    seconds * 1_000_000_000 + nanoseconds
}

/// Whether a `timespec` holds a valid interval.
fn valid_interval(value: &TimeSpec) -> bool {
    value.tv_sec >= 0 && (0..1_000_000_000).contains(&value.tv_nsec)
}

/// Sleeps until `deadline` on the monotonic clock, or until a signal is delivered.
///
/// Returns the nanoseconds left when a signal cut the wait short, and `None` when
/// the deadline was reached. The wait is broken into [`POLL_INTERVAL`] steps because
/// a signal raised on another thread marks itself pending and wakes registered
/// waiters, but the handler runs on this thread the next time it looks — so this
/// thread has to look.
fn sleep_until(deadline: i64) -> Option<i64> {
    // Registering makes a signal raised elsewhere wake this thread's interrupt
    // event, and keeps this wait visible to the signal machinery.
    signal::register_waiter();
    let outcome = loop {
        // Checked before the first sleep, so a signal that arrived just before the
        // call is handled rather than waited out.
        if signal::deliver_pending() != Delivery::None {
            break Some((deadline - monotonic_nanoseconds()).max(0));
        }
        let remaining = deadline - monotonic_nanoseconds();
        if remaining <= 0 {
            break None;
        }
        // The last step is shortened to the remaining time so the sleep does not
        // systematically overshoot by up to the poll interval.
        let step = Duration::from_nanos(remaining as u64).min(POLL_INTERVAL);
        std::thread::sleep(step);
    };
    signal::unregister_waiter();
    outcome
}

/// `nanosleep`.
///
/// Interruptible, and on interruption the time left is written to `remaining` — the
/// behaviour a caller's restart loop depends on, and the reason the value is
/// computed from the monotonic clock rather than from how long the sleep was asked
/// for.
///
/// `EINTR` is returned even when the handler carried `SA_RESTART`. That is not an
/// oversight: Linux never restarts `nanosleep` transparently, because the remaining
/// time lives in the caller's variable and only the caller can resume from it.
///
/// # Safety
///
/// `request` must point at a readable `struct timespec` and `remaining` must be
/// null or point at a writable one.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_nanosleep(
    request: *const TimeSpec,
    remaining: *mut TimeSpec,
) -> c_int {
    if request.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a readable struct.
    let requested = unsafe { ptr::read(request) };
    if !valid_interval(&requested) {
        crate::set_errno(EINVAL);
        return -1;
    }
    ensure_terminate_hook();

    let total = requested.tv_sec * 1_000_000_000 + requested.tv_nsec;
    let deadline = monotonic_nanoseconds() + total;
    match sleep_until(deadline) {
        None => 0,
        Some(left) => {
            if !remaining.is_null() {
                // SAFETY: the caller guarantees a writable struct.
                unsafe {
                    ptr::write(
                        remaining,
                        TimeSpec {
                            tv_sec: left / 1_000_000_000,
                            tv_nsec: left % 1_000_000_000,
                        },
                    );
                }
            }
            crate::set_errno(EINTR);
            -1
        }
    }
}

/// `clock_nanosleep`.
///
/// Returns the error number directly rather than through errno, which is this
/// call's convention and differs from `nanosleep`. `TIMER_ABSTIME` names a deadline
/// on `clock` instead of an interval, and in that mode `remaining` is not written —
/// there is nothing to resume from, since the deadline is already absolute.
///
/// The CPU-time clocks are refused with `EINVAL`: sleeping until a process has
/// consumed a given amount of CPU has no host mechanism, and Linux itself rejects
/// `CLOCK_THREAD_CPUTIME_ID` here.
///
/// # Safety
///
/// `request` must point at a readable `struct timespec` and `remaining` must be
/// null or point at a writable one.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_clock_nanosleep(
    clock: c_int,
    flags: c_int,
    request: *const TimeSpec,
    remaining: *mut TimeSpec,
) -> c_int {
    if request.is_null() {
        return EFAULT;
    }
    if flags & !TIMER_ABSTIME != 0 {
        return EINVAL;
    }
    if matches!(clock, 8 | 9) {
        return EPERM;
    } // Wake alarms need a native wake-capable timer.
    if !matches!(clock, CLOCK_REALTIME | CLOCK_MONOTONIC | CLOCK_BOOTTIME) {
        return EINVAL;
    }
    // SAFETY: the caller guarantees a readable struct.
    let requested = unsafe { ptr::read(request) };
    if !valid_interval(&requested) {
        return EINVAL;
    }
    ensure_terminate_hook();
    // Relative realtime waits ignore wall-clock steps; absolute waits must
    // observe them. Boot waits retain their suspend-inclusive clock throughout.
    let wait_clock = if clock == CLOCK_REALTIME && flags & TIMER_ABSTIME == 0 {
        CLOCK_MONOTONIC
    } else {
        clock
    };
    let now = || read_clock(wait_clock).map(|(s, ns)| s as i128 * 1_000_000_000 + ns as i128);
    let target = requested.tv_sec as i128 * 1_000_000_000 + requested.tv_nsec as i128;
    let deadline = if flags & TIMER_ABSTIME != 0 {
        target
    } else {
        match now() {
            Ok(current) => current + target,
            Err(error) => return error,
        }
    };
    signal::register_waiter();
    let outcome = loop {
        let current = match now() {
            Ok(current) => current,
            Err(error) => {
                signal::unregister_waiter();
                return error;
            }
        };
        let left = (deadline - current).clamp(0, i64::MAX as i128) as i64;
        if signal::deliver_pending() != Delivery::None {
            break Some(left);
        }
        if left == 0 {
            break None;
        }
        std::thread::sleep(Duration::from_nanos(left as u64).min(POLL_INTERVAL));
    };
    signal::unregister_waiter();
    match outcome {
        None => 0,
        Some(left) => {
            // Only the relative form has a remainder worth reporting.
            if flags & TIMER_ABSTIME == 0 && !remaining.is_null() {
                // SAFETY: the caller guarantees a writable struct.
                unsafe {
                    ptr::write(
                        remaining,
                        TimeSpec {
                            tv_sec: left / 1_000_000_000,
                            tv_nsec: left % 1_000_000_000,
                        },
                    );
                }
            }
            EINTR
        }
    }
}

// ---------------------------------------------------------------------------
// Interval timers.
// ---------------------------------------------------------------------------

/// Terminates the process the way a shell reports a fatal signal.
///
/// An unhandled `SIGALRM` terminates on Linux, and a process killed by signal N
/// exits with status `128 + N`. Without this hook installed, `kinakaze-vfs` has no
/// process-lifetime policy to apply and an unhandled alarm would be silently
/// dropped, which would make `sleep 1 &` style code hang rather than exit.
fn terminate_from_signal(signal_number: i32) {
    crate::process::terminate_from_signal(signal_number);
}

/// Installs the default-action hook on first use.
///
/// `set_terminate_hook` keeps the first winner, so this agrees with the identical
/// hook `signal.rs` installs — whichever runs first, the behaviour is the same and
/// repeated calls cost nothing. It is called from every entry point here that can
/// lead to a signal being delivered, because the DLL has no initializer to do it.
fn ensure_terminate_hook() {
    signal::set_terminate_hook(terminate_from_signal);
}

/// The armed `ITIMER_REAL` timer.
struct TimerState {
    /// The monotonic instant of the next expiry, or `None` when disarmed.
    deadline: Option<i64>,
    /// Nanoseconds to reload on expiry. Zero makes the timer one-shot.
    interval: i64,
    /// Bumped on every change so the worker can discard a stale wake-up.
    generation: u64,
    /// Whether the worker thread has been started.
    running: bool,
}

/// The process's interval timer and the worker that fires it.
///
/// **How `SIGALRM` is delivered, and its one limitation.** The worker thread waits
/// until the deadline and calls `kinakaze_vfs::signal::raise_signal`, which is the
/// same entry point `kill` and `raise` use — there is no second mechanism here. That
/// call marks the signal pending and wakes every registered waiter, so a thread
/// parked in `nanosleep`, `sigsuspend` or an interruptible read returns promptly and
/// runs the handler.
///
/// What it cannot do is interrupt a thread that is not looking. Handlers run inside
/// `deliver_pending`, which a thread reaches by making a libc call, so a guest
/// spinning in pure computation sees the alarm only when it next enters this
/// library. On Linux the kernel would divert it mid-instruction. Closing that gap
/// needs either a suspend-and-rewrite-context trick or cooperation from the guest,
/// and neither belongs in a timer; the honest statement is that delivery is prompt
/// for a blocked thread and deferred for a busy one.
///
/// A dedicated thread is used rather than a waitable timer or a timer-queue
/// callback because those deliver on a pool thread this code does not own, and the
/// work to do on expiry — take a lock, reschedule, raise — is the same either way.
static TIMER: OnceLock<(Mutex<TimerState>, Condvar)> = OnceLock::new();

fn timer() -> &'static (Mutex<TimerState>, Condvar) {
    TIMER.get_or_init(|| {
        (
            Mutex::new(TimerState {
                deadline: None,
                interval: 0,
                generation: 0,
                running: false,
            }),
            Condvar::new(),
        )
    })
}

/// The worker loop: waits for the next deadline and raises `SIGALRM` at it.
fn timer_thread() {
    let (state, condition) = timer();
    loop {
        let Ok(mut guard) = state.lock() else {
            // The lock is poisoned, so another thread panicked while holding it and
            // the timer state cannot be trusted. Exiting is better than spinning on
            // a broken invariant; a later `setitimer` starts a fresh worker.
            return;
        };
        let Some(deadline) = guard.deadline else {
            // Disarmed: wait to be armed again. The worker is kept alive rather
            // than exiting so an `alarm` in a loop does not spawn a thread per call.
            let Ok(next) = condition.wait(guard) else {
                return;
            };
            guard = next;
            drop(guard);
            continue;
        };

        let remaining = deadline - monotonic_nanoseconds();
        if remaining > 0 {
            // The generation is captured so a re-arm during the wait is detected
            // rather than the old deadline being fired.
            let generation = guard.generation;
            let Ok((guard_after, _)) =
                condition.wait_timeout(guard, Duration::from_nanos(remaining as u64))
            else {
                return;
            };
            let mut guard = guard_after;
            if guard.generation != generation {
                // Re-armed while waiting; the loop re-reads the new deadline.
                drop(guard);
                continue;
            }
            // Woken early without a change: loop and re-check the clock rather than
            // trusting the wait's own timeout, since a condvar may wake spuriously.
            if guard.deadline == Some(deadline) && monotonic_nanoseconds() < deadline {
                drop(guard);
                continue;
            }
            guard.deadline = None;
            let interval = guard.interval;
            if interval > 0 {
                // A periodic timer reloads from the deadline, not from now, so the
                // period does not drift by the delivery latency each time.
                guard.deadline = Some(deadline + interval);
                guard.generation = guard.generation.wrapping_add(1);
            }
            drop(guard);
        } else {
            guard.deadline = None;
            let interval = guard.interval;
            if interval > 0 {
                guard.deadline = Some(deadline + interval);
                guard.generation = guard.generation.wrapping_add(1);
            }
            drop(guard);
        }

        // Raised outside the lock: a handler may call back into `setitimer`, and
        // holding the lock across it would deadlock.
        let _ = signal::raise_signal(SIGALRM);
    }
}

/// Arms or disarms the timer, returning the nanoseconds that were left.
fn arm_timer(deadline: Option<i64>, interval: i64) -> (i64, i64) {
    ensure_terminate_hook();
    let (state, condition) = timer();
    let Ok(mut guard) = state.lock() else {
        return (0, 0);
    };
    let now = monotonic_nanoseconds();
    // What was left, which both `alarm` and `setitimer` have to report.
    let previous_remaining = guard.deadline.map_or(0, |deadline| (deadline - now).max(0));
    let previous_interval = guard.interval;

    guard.deadline = deadline;
    guard.interval = interval;
    guard.generation = guard.generation.wrapping_add(1);
    let start = !guard.running && deadline.is_some();
    if start {
        guard.running = true;
    }
    drop(guard);

    if start {
        // Spawned on first use rather than at load time, so a guest that never sets
        // a timer never pays for the thread.
        let _ = std::thread::Builder::new()
            .name(String::from("kinakaze-itimer"))
            .spawn(timer_thread);
    }
    // Wakes the worker so a shortened deadline takes effect immediately.
    condition.notify_all();
    (previous_remaining, previous_interval)
}

/// `alarm`.
///
/// Returns the seconds left on any previous alarm, **rounded up**: a caller told 0
/// would conclude no alarm was pending, so a partial second reports as 1. That is
/// what Linux does. An argument of 0 cancels the timer and reports what was left.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_alarm(seconds: c_uint) -> c_uint {
    let requested = i64::from(seconds);
    let deadline = if requested == 0 {
        None
    } else {
        Some(monotonic_nanoseconds() + requested * 1_000_000_000)
    };
    // `alarm` and `setitimer(ITIMER_REAL)` are the same timer on Linux, so setting
    // one cancels the other. Sharing the state here is what preserves that.
    let (remaining, _) = arm_timer(deadline, 0);
    // Rounded up, per the note above.
    ((remaining + 999_999_999) / 1_000_000_000) as c_uint
}

/// `setitimer`.
///
/// `ITIMER_REAL` is implemented and is the same timer `alarm` uses, so the two
/// cancel each other exactly as on Linux.
///
/// `ITIMER_VIRTUAL` and `ITIMER_PROF` report `ENOSYS`. Both measure CPU time rather
/// than elapsed time, and while `GetProcessTimes` could be polled to approximate
/// them, the result would be a timer whose expiry lags by the polling interval under
/// exactly the load that makes CPU time accumulate — which is when a profiler cares
/// most. `ENOSYS` is a condition callers already handle, and reporting a timer that
/// fires late and unevenly would be the worse answer.
///
/// # Safety
///
/// `value` must be null or point at a readable `struct itimerval`, and `old_value`
/// must be null or point at a writable one.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setitimer(
    which: c_int,
    value: *const ItimerVal,
    old_value: *mut ItimerVal,
) -> c_int {
    if which == ITIMER_VIRTUAL || which == ITIMER_PROF {
        crate::set_errno(ENOSYS);
        return -1;
    }
    if which != ITIMER_REAL {
        crate::set_errno(EINVAL);
        return -1;
    }
    if value.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a readable struct.
    let requested = unsafe { ptr::read(value) };
    // A microsecond field outside 0..1e6 is EINVAL, as it is on Linux.
    let usable = |slot: &TimeVal| slot.tv_sec >= 0 && (0..1_000_000).contains(&slot.tv_usec);
    if !usable(&requested.it_value) || !usable(&requested.it_interval) {
        crate::set_errno(EINVAL);
        return -1;
    }

    let nanoseconds = |slot: &TimeVal| slot.tv_sec * 1_000_000_000 + slot.tv_usec * 1000;
    let initial = nanoseconds(&requested.it_value);
    let interval = nanoseconds(&requested.it_interval);
    // An `it_value` of zero disarms the timer, even with a non-zero interval.
    let deadline = if initial == 0 {
        None
    } else {
        Some(monotonic_nanoseconds() + initial)
    };

    let (remaining, previous_interval) =
        arm_timer(deadline, if initial == 0 { 0 } else { interval });

    if !old_value.is_null() {
        let to_timeval = |nanoseconds: i64| TimeVal {
            tv_sec: nanoseconds / 1_000_000_000,
            tv_usec: (nanoseconds % 1_000_000_000) / 1000,
        };
        // SAFETY: the caller guarantees a writable struct.
        unsafe {
            ptr::write(
                old_value,
                ItimerVal {
                    it_interval: to_timeval(previous_interval),
                    it_value: to_timeval(remaining),
                },
            );
        }
    }
    0
}

/// `getitimer`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getitimer(
    which: c_int,
    curr_value: *mut ItimerVal,
) -> c_int {
    if which == ITIMER_VIRTUAL || which == ITIMER_PROF {
        crate::set_errno(ENOSYS);
        return -1;
    }
    if which != ITIMER_REAL {
        crate::set_errno(EINVAL);
        return -1;
    }
    if curr_value.is_null() {
        crate::set_errno(EFAULT);
        return -1;
    }
    let (remaining, interval) = arm_timer(None, 0);
    let deadline = if remaining > 0 {
        Some(monotonic_nanoseconds() + remaining)
    } else {
        None
    };
    let _ = arm_timer(deadline, interval);
    let to_timeval = |nanoseconds: i64| TimeVal {
        tv_sec: nanoseconds / 1_000_000_000,
        tv_usec: (nanoseconds % 1_000_000_000) / 1000,
    };
    unsafe {
        ptr::write(
            curr_value,
            ItimerVal {
                it_interval: to_timeval(interval),
                it_value: to_timeval(remaining),
            },
        );
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_clock_write_requires_guest_cap_sys_time() {
        // The current capability provider grants no CAP_SYS_TIME. This tests
        // authorization without asking Windows to mutate its clock.
        assert_eq!(
            crate::userdb::effective_capabilities().unwrap() & (1 << 25),
            0
        );
        assert_eq!(require_clock_write_capability(), Err(EPERM));
    }
    use core::mem::offset_of;

    /// A reference instant used throughout: 1700000000 is
    /// **2023-11-14 22:13:20 UTC**, a Tuesday, and the 318th day of the year.
    /// Every expected value below was worked out from the calendar, not from this
    /// module's own output.
    const REFERENCE: Time = 1_700_000_000;

    /// Builds a UTC `struct tm` for `clock` through the real entry point.
    fn utc_tm(clock: Time) -> Tm {
        let mut tm = Tm::default();
        // SAFETY: both pointers address writable locals.
        let result = unsafe { kinakaze_abi_gmtime_r(&raw const clock, &raw mut tm) };
        assert!(!result.is_null(), "gmtime_r rejected a valid time_t");
        tm
    }

    /// Formats `tm` with `format` and returns the result as a `String`.
    fn format(tm: &Tm, format: &str) -> String {
        let pattern = CString::new(format).expect("format holds no NUL");
        let mut buffer = [0i8; 512];
        // SAFETY: the buffer is writable for its whole length and both the format
        // and the struct are valid.
        let written = unsafe {
            kinakaze_abi_strftime(
                buffer.as_mut_ptr().cast::<c_char>(),
                buffer.len(),
                pattern.as_ptr(),
                &raw const *tm,
            )
        };
        // SAFETY: strftime NUL-terminates on success.
        let text = unsafe { CStr::from_ptr(buffer.as_ptr().cast::<c_char>()) }
            .to_str()
            .expect("strftime produced invalid UTF-8")
            .to_string();
        assert_eq!(
            written,
            text.len(),
            "the returned length must match the string"
        );
        text
    }

    #[test]
    fn struct_layouts_match_the_linux_abi() {
        // A guest computed these offsets against glibc's headers at its own compile
        // time, so a mismatch here is silent corruption rather than a build error.
        assert_eq!(size_of::<TimeSpec>(), 16);
        assert_eq!(offset_of!(TimeSpec, tv_nsec), 8);
        assert_eq!(size_of::<TimeVal>(), 16);
        assert_eq!(offset_of!(TimeVal, tv_usec), 8);

        // The padding case. Nine ints end at 36, and `tm_gmtoff` needs 8-byte
        // alignment, so it lands at 40 and the struct is 56 bytes rather than 48.
        assert_eq!(offset_of!(Tm, tm_sec), 0);
        assert_eq!(offset_of!(Tm, tm_min), 4);
        assert_eq!(offset_of!(Tm, tm_hour), 8);
        assert_eq!(offset_of!(Tm, tm_mday), 12);
        assert_eq!(offset_of!(Tm, tm_mon), 16);
        assert_eq!(offset_of!(Tm, tm_year), 20);
        assert_eq!(offset_of!(Tm, tm_wday), 24);
        assert_eq!(offset_of!(Tm, tm_yday), 28);
        assert_eq!(offset_of!(Tm, tm_isdst), 32);
        assert_eq!(
            offset_of!(Tm, tm_gmtoff),
            40,
            "4 bytes of padding precede this"
        );
        assert_eq!(offset_of!(Tm, tm_zone), 48);
        assert_eq!(size_of::<Tm>(), 56);
        assert_eq!(align_of::<Tm>(), 8);

        assert_eq!(size_of::<ItimerVal>(), 32);
        assert_eq!(offset_of!(ItimerVal, it_value), 16);

        // Four `clock_t`, each a 64-bit `long`.
        assert_eq!(size_of::<Tms>(), 32);
        assert_eq!(offset_of!(Tms, tms_stime), 8);
        assert_eq!(offset_of!(Tms, tms_cutime), 16);
        assert_eq!(offset_of!(Tms, tms_cstime), 24);

        // The kernel's `__kernel_timex`, including its trailing reserved space.
        assert_eq!(
            offset_of!(Timex, offset),
            8,
            "padding follows the modes int"
        );
        assert_eq!(offset_of!(Timex, status), 40);
        assert_eq!(offset_of!(Timex, time), 72);
        assert_eq!(offset_of!(Timex, tick), 88);
        assert_eq!(offset_of!(Timex, tai), 160);
        assert_eq!(size_of::<Timex>(), 208);
    }

    #[test]
    fn no_abi_type_is_the_hosts_long() {
        // The trap this module was written around: on Windows MSVC `c_long` is 32
        // bits, so a `time_t` declared with it would return half a value into the
        // register the guest reads.
        assert_eq!(size_of::<Time>(), 8, "time_t is a 64-bit long on Linux");
        assert_eq!(size_of::<Long>(), 8);
        assert_eq!(
            size_of::<Time>(),
            size_of::<i64>(),
            "clock_t and suseconds_t are the same width"
        );
    }

    #[test]
    fn gmtime_r_matches_the_calendar() {
        let tm = utc_tm(REFERENCE);
        // 2023-11-14 22:13:20 UTC.
        assert_eq!(tm.tm_year, 123, "tm_year counts from 1900");
        assert_eq!(tm.tm_mon, 10, "November is month 10 zero-based");
        assert_eq!(tm.tm_mday, 14);
        assert_eq!(tm.tm_hour, 22);
        assert_eq!(tm.tm_min, 13);
        assert_eq!(tm.tm_sec, 20);
        // 2023-11-14 was a Tuesday.
        assert_eq!(tm.tm_wday, 2, "Tuesday, counting Sunday as 0");
        // 304 days precede November, plus 13 → zero-based 317.
        assert_eq!(tm.tm_yday, 317, "tm_yday is zero-based");
        // UTC never observes DST, and the offset is zero by definition.
        assert_eq!(tm.tm_isdst, 0);
        assert_eq!(tm.tm_gmtoff, 0);
        // SAFETY: `gmtime_r` always publishes a live interned string here.
        assert_eq!(unsafe { CStr::from_ptr(tm.tm_zone) }.to_str(), Ok("UTC"));

        // The epoch itself, and a pre-epoch instant, which uses the negative
        // branch of every division in the conversion.
        let epoch = utc_tm(0);
        assert_eq!((epoch.tm_year, epoch.tm_mon, epoch.tm_mday), (70, 0, 1));
        assert_eq!(epoch.tm_wday, 4, "1970-01-01 was a Thursday");
        assert_eq!(epoch.tm_yday, 0);

        let before = utc_tm(-1);
        assert_eq!(
            (before.tm_year, before.tm_mon, before.tm_mday),
            (69, 11, 31),
            "one second before the epoch is 1969-12-31"
        );
        assert_eq!((before.tm_hour, before.tm_min, before.tm_sec), (23, 59, 59));
        assert_eq!(before.tm_yday, 364, "1969 was not a leap year");
    }

    #[test]
    fn timegm_and_mktime_round_trip_through_gmtime() {
        for clock in [
            0,
            REFERENCE,
            // The leap day, which is where a wrong February length shows up.
            1_709_164_800,
            // 2021-01-01 00:00:00 UTC, a year boundary.
            1_609_459_200,
            // 2000-02-29, the century leap year the 400-rule rescues.
            951_782_400,
            // Pre-epoch.
            -86_400,
        ] {
            let mut tm = utc_tm(clock);
            // SAFETY: `tm` is a writable local.
            let round_tripped = unsafe { kinakaze_abi_timegm(&raw mut tm) };
            assert_eq!(round_tripped, clock, "timegm must invert gmtime_r");
            // The struct is written back, so the derived fields must survive.
            let fresh = utc_tm(clock);
            assert_eq!(tm.tm_wday, fresh.tm_wday);
            assert_eq!(tm.tm_yday, fresh.tm_yday);
        }

        // The leap day specifically.
        let leap = utc_tm(1_709_164_800);
        assert_eq!(
            (leap.tm_year, leap.tm_mon, leap.tm_mday),
            (124, 1, 29),
            "2024-02-29 must exist"
        );
        assert_eq!(leap.tm_wday, 4, "2024-02-29 was a Thursday");
        assert_eq!(leap.tm_yday, 59, "31 January days plus 28 February days");

        // `mktime` inverts `localtime_r` for the same instant, whatever the local
        // zone is — which is the property that has to hold rather than any
        // particular offset, since the test machine's zone is not known here.
        for clock in [0, REFERENCE, 1_709_164_800, 1_609_459_200] {
            let mut tm = Tm::default();
            // SAFETY: both pointers address writable locals.
            let result = unsafe { kinakaze_abi_localtime_r(&raw const clock, &raw mut tm) };
            assert!(!result.is_null());
            // SAFETY: `tm` is a writable local.
            let back = unsafe { kinakaze_abi_mktime(&raw mut tm) };
            assert_eq!(back, clock, "mktime must invert localtime_r");
        }
    }

    #[test]
    fn mktime_normalises_out_of_range_fields() {
        /// Normalises a UTC date through `timegm`, which shares the arithmetic
        /// with `mktime` but needs no zone to make the result predictable.
        fn normalise(
            year: c_int,
            month: c_int,
            day: c_int,
            hour: c_int,
        ) -> (c_int, c_int, c_int, c_int) {
            let mut tm = Tm {
                tm_year: year - 1900,
                tm_mon: month,
                tm_mday: day,
                tm_hour: hour,
                // Deliberately garbage: both are output-only and must be rewritten.
                tm_wday: 99,
                tm_yday: 99,
                ..Tm::default()
            };
            // SAFETY: `tm` is a writable local.
            unsafe { kinakaze_abi_timegm(&raw mut tm) };
            (tm.tm_year + 1900, tm.tm_mon, tm.tm_mday, tm.tm_hour)
        }

        // Month 13 one-based, so `tm_mon` 12, rolls into the next year.
        assert_eq!(normalise(2023, 12, 1, 0), (2024, 0, 1, 0));
        // Day 0 is the last day of the previous month.
        assert_eq!(normalise(2023, 0, 0, 0), (2022, 11, 31, 0));
        // Hour 24 is midnight the following day.
        assert_eq!(normalise(2023, 0, 1, 24), (2023, 0, 2, 0));
        // A day past the end of February in a leap year lands on March 1st.
        assert_eq!(normalise(2024, 1, 30, 0), (2024, 2, 1, 0));
        // And in a common year, February 30th is March 2nd.
        assert_eq!(normalise(2023, 1, 30, 0), (2023, 2, 2, 0));
        // Negative months count backwards.
        assert_eq!(normalise(2023, -1, 1, 0), (2022, 11, 1, 0));

        // The derived fields are recomputed rather than left as the caller had them.
        let mut tm = Tm {
            tm_year: 123,
            tm_mon: 12,
            tm_mday: 1,
            tm_wday: 99,
            tm_yday: 99,
            ..Tm::default()
        };
        // SAFETY: `tm` is a writable local.
        unsafe { kinakaze_abi_timegm(&raw mut tm) };
        assert_eq!(tm.tm_wday, 1, "2024-01-01 was a Monday");
        assert_eq!(tm.tm_yday, 0);
    }

    #[test]
    fn iso_week_numbering_is_not_week_of_year() {
        // The case that catches an implementation aliasing %V onto %U or %W:
        // 2021-01-01 is a Friday, and ISO puts it in week 53 of 2020.
        let tm = utc_tm(1_609_459_200);
        assert_eq!(tm.tm_wday, 5, "2021-01-01 was a Friday");
        assert_eq!(format(&tm, "%V"), "53");
        assert_eq!(
            format(&tm, "%G"),
            "2020",
            "the ISO year is the previous one"
        );
        assert_eq!(format(&tm, "%g"), "20");
        // While the calendar year and the week-of-year conversions disagree with it.
        assert_eq!(format(&tm, "%Y"), "2021");
        assert_eq!(format(&tm, "%U"), "00");
        assert_eq!(format(&tm, "%W"), "00");

        // 2020 is a long ISO year; 2021 and 2023 are not.
        assert_eq!(iso_weeks_in_year(2020), 53);
        assert_eq!(iso_weeks_in_year(2021), 52);
        assert_eq!(iso_weeks_in_year(2023), 52);
        // 2004 ended on a Friday but began on a Thursday, making it long too.
        assert_eq!(iso_weeks_in_year(2004), 53);

        // 2019-12-30 is a Monday and belongs to ISO week 1 of 2020.
        let crossing = utc_tm(1_577_664_000);
        assert_eq!(
            (crossing.tm_year, crossing.tm_mon, crossing.tm_mday),
            (119, 11, 30)
        );
        assert_eq!(format(&crossing, "%V"), "01");
        assert_eq!(
            format(&crossing, "%G"),
            "2020",
            "the ISO year runs ahead here"
        );

        // A mid-year date, where all three week conversions agree.
        let ordinary = utc_tm(REFERENCE);
        assert_eq!(format(&ordinary, "%V"), "46");
        assert_eq!(format(&ordinary, "%U"), "46");
        assert_eq!(format(&ordinary, "%W"), "46");
        assert_eq!(format(&ordinary, "%G"), "2023");
    }

    #[test]
    fn strftime_renders_every_supported_conversion() {
        let tm = utc_tm(REFERENCE);

        // Hand-written expectations for 2023-11-14 22:13:20 UTC, a Tuesday.
        assert_eq!(format(&tm, "%Y-%m-%d"), "2023-11-14");
        assert_eq!(format(&tm, "%H:%M:%S"), "22:13:20");
        assert_eq!(
            format(&tm, "%j"),
            "318",
            "%j is one-based where tm_yday is not"
        );
        assert_eq!(format(&tm, "%u"), "2", "Monday is 1, so Tuesday is 2");
        assert_eq!(format(&tm, "%w"), "2", "and Sunday is 0 here");

        assert_eq!(format(&tm, "%a"), "Tue");
        assert_eq!(format(&tm, "%A"), "Tuesday");
        assert_eq!(format(&tm, "%b"), "Nov");
        assert_eq!(format(&tm, "%h"), "Nov", "%h is a synonym for %b");
        assert_eq!(format(&tm, "%B"), "November");
        assert_eq!(format(&tm, "%C"), "20");
        assert_eq!(format(&tm, "%y"), "23");
        assert_eq!(format(&tm, "%e"), "14");
        assert_eq!(format(&tm, "%k"), "22");

        // 22:13 in 12-hour terms.
        assert_eq!(format(&tm, "%I"), "10");
        assert_eq!(format(&tm, "%l"), "10");
        assert_eq!(format(&tm, "%p"), "PM");
        assert_eq!(format(&tm, "%P"), "pm");

        // The composites.
        assert_eq!(format(&tm, "%D"), "11/14/23");
        assert_eq!(format(&tm, "%F"), "2023-11-14");
        assert_eq!(format(&tm, "%T"), "22:13:20");
        assert_eq!(format(&tm, "%R"), "22:13");
        assert_eq!(format(&tm, "%r"), "10:13:20 PM");
        assert_eq!(format(&tm, "%c"), "Tue Nov 14 22:13:20 2023");
        assert_eq!(format(&tm, "%x"), "11/14/23");
        assert_eq!(format(&tm, "%X"), "22:13:20");

        // Zone conversions, which for a UTC struct are exactly determined.
        assert_eq!(format(&tm, "%z"), "+0000");
        assert_eq!(format(&tm, "%Z"), "UTC");
        // `%s` reverses the offset the struct carries, which is zero here.
        assert_eq!(format(&tm, "%s"), "1700000000");

        // Literals and escapes.
        assert_eq!(format(&tm, "%n"), "\n");
        assert_eq!(format(&tm, "%t"), "\t");
        assert_eq!(format(&tm, "%%"), "%");
        assert_eq!(format(&tm, "a%%b"), "a%b");
        // An unrecognized conversion survives verbatim rather than vanishing.
        assert_eq!(format(&tm, "%Q"), "%Q");
    }

    #[test]
    fn strftime_honours_flags_widths_and_modifiers() {
        // 2021-01-01, where the single-digit day and month make padding visible.
        let tm = utc_tm(1_609_459_200);

        assert_eq!(format(&tm, "%d"), "01", "%d zero-pads by default");
        assert_eq!(format(&tm, "%e"), " 1", "%e space-pads by default");
        assert_eq!(format(&tm, "%-d"), "1", "the - flag removes padding");
        assert_eq!(format(&tm, "%_d"), " 1", "the _ flag pads with a space");
        assert_eq!(format(&tm, "%0e"), "01", "the 0 flag pads with a zero");
        assert_eq!(format(&tm, "%-j"), "1");
        assert_eq!(format(&tm, "%j"), "001");

        // Widths.
        assert_eq!(format(&tm, "%6Y"), "002021");
        assert_eq!(format(&tm, "%4Y"), "2021");
        assert_eq!(format(&tm, "%_6d"), "     1");

        // Case flags on a text conversion.
        assert_eq!(format(&tm, "%^a"), "FRI");
        assert_eq!(format(&tm, "%^B"), "JANUARY");
        assert_eq!(format(&tm, "%#a"), "fRI", "# swaps the case of each byte");
        assert_eq!(format(&tm, "%^p"), "AM");

        // The E and O modifiers name locale alternatives the C locale lacks, so
        // they are consumed and the base conversion applied — as glibc does.
        assert_eq!(format(&tm, "%EY"), "2021");
        assert_eq!(format(&tm, "%Od"), "01");
        assert_eq!(format(&tm, "%Ey"), "21");

        // Midnight in 12-hour form is 12 AM, not 0.
        assert_eq!(format(&tm, "%I"), "12");
        assert_eq!(format(&tm, "%p"), "AM");
        assert_eq!(format(&tm, "%P"), "am");
        assert_eq!(format(&tm, "%k"), " 0");
    }

    #[test]
    fn strftime_reports_a_buffer_that_is_too_small() {
        let tm = utc_tm(REFERENCE);
        let pattern = CString::new("%Y-%m-%d").expect("no NUL");
        // The output is 10 bytes, so 10 is still too small: the terminator must fit.
        let mut buffer = [0i8; 10];
        // SAFETY: the buffer is writable for its whole length.
        let written = unsafe {
            kinakaze_abi_strftime(
                buffer.as_mut_ptr().cast::<c_char>(),
                buffer.len(),
                pattern.as_ptr(),
                &raw const tm,
            )
        };
        assert_eq!(written, 0, "a result that cannot be terminated reports 0");

        let mut buffer = [0i8; 11];
        // SAFETY: as above.
        let written = unsafe {
            kinakaze_abi_strftime(
                buffer.as_mut_ptr().cast::<c_char>(),
                buffer.len(),
                pattern.as_ptr(),
                &raw const tm,
            )
        };
        assert_eq!(written, 10, "one more byte is exactly enough");
    }

    /// Parses `input` with `format`, returning the struct and the unparsed tail.
    fn parse(input: &str, format: &str) -> Option<(Tm, String)> {
        let text = CString::new(input).expect("input holds no NUL");
        let pattern = CString::new(format).expect("format holds no NUL");
        let mut tm = Tm::default();
        // SAFETY: both strings are null-terminated and `tm` is a writable local.
        let rest = unsafe { kinakaze_abi_strptime(text.as_ptr(), pattern.as_ptr(), &raw mut tm) };
        if rest.is_null() {
            return None;
        }
        // SAFETY: the returned pointer addresses the tail of `text`, which is
        // null-terminated.
        let tail = unsafe { CStr::from_ptr(rest) }
            .to_str()
            .expect("the tail is a suffix of valid UTF-8")
            .to_string();
        Some((tm, tail))
    }

    #[test]
    fn strptime_parses_back_what_strftime_produced() {
        let original = utc_tm(REFERENCE);
        // Each of these formats carries enough information to rebuild the instant.
        for pattern in [
            "%Y-%m-%d %H:%M:%S",
            "%F %T",
            "%c",
            "%d/%m/%Y %H:%M:%S",
            "%Y %j %T",
        ] {
            let rendered = format(&original, pattern);
            let (mut parsed, tail) = parse(&rendered, pattern)
                .unwrap_or_else(|| panic!("{pattern:?} failed to parse {rendered:?}"));
            assert!(tail.is_empty(), "{pattern:?} left {tail:?} unconsumed");

            // `%Y %j %T` carries the day of year rather than a month and day, and
            // nothing here reconstructs the date from it — glibc does not either.
            // So `tm_yday` is what survives, and it is checked before `timegm`,
            // which recomputes that field from the month and day it was not given.
            if pattern == "%Y %j %T" {
                assert_eq!(parsed.tm_yday, original.tm_yday, "%j must survive");
                assert_eq!(parsed.tm_hour, original.tm_hour);
                assert_eq!(parsed.tm_year, original.tm_year);
                continue;
            }
            // SAFETY: `parsed` is a writable local.
            let instant = unsafe { kinakaze_abi_timegm(&raw mut parsed) };
            assert_eq!(instant, REFERENCE, "{pattern:?} did not round-trip");
            assert_eq!(parsed.tm_wday, original.tm_wday, "{pattern:?} lost tm_wday");
            assert_eq!(parsed.tm_yday, original.tm_yday, "{pattern:?} lost tm_yday");
        }
    }

    #[test]
    fn strptime_handles_the_awkward_conversions() {
        // The 12-hour clock needs both halves, and they may appear in either order.
        let (tm, _) = parse("10:13 PM", "%I:%M %p").expect("12-hour form parses");
        assert_eq!(tm.tm_hour, 22);
        assert_eq!(tm.tm_min, 13);
        let (tm, _) = parse("PM 10", "%p %I").expect("the meridiem may come first");
        assert_eq!(tm.tm_hour, 22);
        // 12 AM is hour 0 and 12 PM is hour 12.
        let (tm, _) = parse("12:00 AM", "%I:%M %p").expect("midnight parses");
        assert_eq!(tm.tm_hour, 0);
        let (tm, _) = parse("12:00 PM", "%I:%M %p").expect("noon parses");
        assert_eq!(tm.tm_hour, 12);

        // The POSIX century rule for a two-digit year.
        let (tm, _) = parse("69", "%y").expect("a two-digit year parses");
        assert_eq!(tm.tm_year, 69, "69 means 1969");
        let (tm, _) = parse("68", "%y").expect("a two-digit year parses");
        assert_eq!(tm.tm_year, 168, "68 means 2068");
        // An explicit century overrides the rule.
        let (tm, _) = parse("18 99", "%C %y").expect("century and year parse");
        assert_eq!(tm.tm_year, -1, "1899 is 1900 less one");

        // `%s` names an absolute instant and determines every field.
        let (tm, _) = parse("1700000000", "%s").expect("epoch seconds parse");
        assert_eq!((tm.tm_year, tm.tm_mon, tm.tm_mday), (123, 10, 14));
        assert_eq!(tm.tm_hour, 22);
        let (tm, _) = parse("-86400", "%s").expect("a negative instant parses");
        assert_eq!((tm.tm_year, tm.tm_mon, tm.tm_mday), (69, 11, 31));

        // `%z` in each spelling it appears in.
        for (input, expected) in [
            ("+0530", 19_800),
            ("+05:30", 19_800),
            ("-0800", -28_800),
            ("+09", 32_400),
            ("Z", 0),
        ] {
            let (tm, _) = parse(input, "%z")
                .unwrap_or_else(|| panic!("{input:?} should parse as a zone offset"));
            assert_eq!(tm.tm_gmtoff, expected, "{input:?} gave the wrong offset");
        }

        // Names are matched case-insensitively, and the longest match wins so a
        // full name is not accepted as an abbreviation with a stray tail.
        let (tm, tail) = parse("september", "%b").expect("lower case parses");
        assert_eq!(tm.tm_mon, 8);
        assert!(
            tail.is_empty(),
            "the full name must be consumed, not just Sep"
        );
        let (tm, _) = parse("TUESDAY", "%A").expect("upper case parses");
        assert_eq!(tm.tm_wday, 2);

        // Whitespace in the format matches any run of it, including none.
        assert!(
            parse("Nov  14", "%b %d").is_some(),
            "extra whitespace is allowed"
        );
        assert!(parse("Nov14", "%b %d").is_some(), "so is none");

        // The tail is reported so a caller can continue parsing.
        let (_, tail) = parse("2023-11-14 and the rest", "%Y-%m-%d").expect("prefix parses");
        assert_eq!(tail, " and the rest");

        // Failures report null rather than a partial result.
        assert!(parse("not a year", "%Y").is_none());
        assert!(
            parse("2023-11-14", "%Y/%m/%d").is_none(),
            "a literal must match"
        );
        assert!(
            parse("13/01/2023", "%m/%d/%Y").is_none(),
            "month 13 is rejected"
        );
        assert!(
            parse("00:60:00", "%H:%M:%S").is_none(),
            "minute 60 is rejected"
        );
        // A leap second is accepted, as it is on Linux.
        assert!(parse("23:59:60", "%H:%M:%S").is_some());

        // Fields the format does not mention keep the values they arrived with,
        // which is what makes the incremental two-call idiom work.
        let text = CString::new("22:13:20").expect("no NUL");
        let pattern = CString::new("%H:%M:%S").expect("no NUL");
        let mut tm = utc_tm(REFERENCE);
        tm.tm_hour = 0;
        // SAFETY: both strings are null-terminated and `tm` is a writable local.
        let rest = unsafe { kinakaze_abi_strptime(text.as_ptr(), pattern.as_ptr(), &raw mut tm) };
        assert!(!rest.is_null());
        assert_eq!(tm.tm_mday, 14, "the date the caller supplied must survive");
        assert_eq!(tm.tm_hour, 22, "and the time must be updated");
    }

    #[test]
    fn tz_parsing_gets_the_sign_convention_right() {
        // A POSIX TZ offset is west-positive; `tm_gmtoff` is east-positive. `EST5`
        // is five hours *behind* UTC, so the offset is negative.
        let Some(Rules::Fixed { offset, abbrev }) = parse_tz("EST5") else {
            panic!("EST5 should parse as a fixed zone");
        };
        assert_eq!(offset, -18_000, "west-positive in, east-positive out");
        assert_eq!(abbrev.as_str(), "EST");

        // A bare name means UTC under that name, and an empty TZ means UTC.
        for value in ["UTC", "GMT", "UTC0", ""] {
            let Some(Rules::Fixed { offset, .. }) = parse_tz(value) else {
                panic!("{value:?} should parse as a fixed zone");
            };
            assert_eq!(offset, 0, "{value:?} should be UTC");
        }

        // A zone east of UTC.
        let Some(Rules::Fixed { offset, .. }) = parse_tz("IST-5:30") else {
            panic!("IST-5:30 should parse");
        };
        assert_eq!(offset, 19_800, "India is five and a half hours ahead");

        // The bracketed form exists so an abbreviation may contain digits.
        let Some(Rules::Fixed { abbrev, offset }) = parse_tz("<+04>-4") else {
            panic!("the bracketed form should parse");
        };
        assert_eq!(abbrev.as_str(), "+04");
        assert_eq!(offset, 14_400);

        // A zoneinfo reference cannot be resolved without the IANA database, so it
        // reports no rules and the caller falls back to the Windows zone.
        assert!(parse_tz("Europe/Berlin").is_none());
        assert!(parse_tz(":/etc/localtime").is_none());
        // So does a malformed value, rather than half of it taking effect.
        assert!(
            parse_tz("EST5EDT,M3.2.0").is_none(),
            "one rule of two is not enough"
        );
        assert!(
            parse_tz("XX5").is_none(),
            "an abbreviation needs three characters"
        );
    }

    #[test]
    fn tz_daylight_rules_select_the_right_offset() {
        let rules = parse_tz("EST5EDT,M3.2.0,M11.1.0").expect("US eastern rules parse");

        // 2023-01-15 12:00 UTC: winter, so standard time.
        let winter = offset_at(rules, 1_673_784_000);
        assert_eq!(winter.seconds, -18_000);
        assert!(!winter.daylight);
        assert_eq!(winter.abbrev.as_str(), "EST");

        // 2023-07-15 12:00 UTC: summer, so daylight time.
        let summer = offset_at(rules, 1_689_422_400);
        assert_eq!(summer.seconds, -14_400);
        assert!(summer.daylight);
        assert_eq!(summer.abbrev.as_str(), "EDT");

        // The transitions themselves. DST began 2023-03-12 at 02:00 EST, which is
        // 07:00 UTC, so one second earlier is still standard time.
        let start = 1_678_604_400;
        assert!(
            !offset_at(rules, start - 1).daylight,
            "02:00 EST has not arrived"
        );
        assert!(offset_at(rules, start).daylight, "and at 02:00 EST it has");

        // A southern-hemisphere zone, where the daylight interval wraps New Year
        // and the containment test has to invert.
        let southern = parse_tz("AEST-10AEDT,M10.1.0,M4.1.0").expect("Australian rules parse");
        // 2023-01-15: southern summer, so daylight time.
        assert!(offset_at(southern, 1_673_784_000).daylight);
        assert_eq!(offset_at(southern, 1_673_784_000).seconds, 39_600);
        // 2023-07-15: southern winter, so standard time.
        assert!(!offset_at(southern, 1_689_422_400).daylight);
        assert_eq!(offset_at(southern, 1_689_422_400).seconds, 36_000);

        // The `Jn` and `n` transition forms, which skip and count February 29th.
        let julian = parse_tz("XXX0YYY,J60/0,J300/0").expect("Julian rules parse");
        // In 2024, J60 is March 1st because February 29th is unnumbered.
        assert_eq!(
            Transition::JulianNoLeap { day: 60, time: 0 }.local_seconds(2024),
            days_from_civil(2024, 3, 1) * SECONDS_PER_DAY,
            "J60 skips the leap day"
        );
        // While the zero-based form counts it, so day 60 is February 29th.
        assert_eq!(
            Transition::ZeroBasedDay { day: 59, time: 0 }.local_seconds(2024),
            days_from_civil(2024, 2, 29) * SECONDS_PER_DAY,
            "the zero-based form counts the leap day"
        );
        assert!(matches!(julian, Rules::Posix { .. }));
    }

    #[test]
    fn local_time_conversions_agree_with_the_offset_they_report() {
        // The machine's zone is not known here, so the property tested is the
        // invariant that must hold in any zone: the local fields, read back as if
        // they were UTC, differ from the true instant by exactly `tm_gmtoff`.
        for clock in [0, REFERENCE, 1_609_459_200, 1_689_422_400] {
            let mut local = Tm::default();
            // SAFETY: both pointers address writable locals.
            let result = unsafe { kinakaze_abi_localtime_r(&raw const clock, &raw mut local) };
            assert!(!result.is_null());
            let (year, month, day, hour, minute, second) = fields_of(&local);
            let as_utc = seconds_from_fields(year, month, day, hour, minute, second);
            assert_eq!(
                as_utc - local.tm_gmtoff,
                clock,
                "the local fields and tm_gmtoff must describe the same instant"
            );
            // A real zone is within fifteen hours of UTC in either direction.
            assert!(
                (-15 * 3600..=15 * 3600).contains(&local.tm_gmtoff),
                "tm_gmtoff {} is not a real offset — check the sign convention",
                local.tm_gmtoff
            );
            assert!(!local.tm_zone.is_null(), "tm_zone must be a usable string");
        }

        // The non-reentrant forms return usable per-thread storage.
        let clock = REFERENCE;
        // SAFETY: `clock` is a readable local.
        let published = unsafe { kinakaze_abi_gmtime(&raw const clock) };
        assert!(!published.is_null());
        // SAFETY: the pointer addresses this thread's buffer.
        assert_eq!(unsafe { (*published).tm_mday }, 14);
        // SAFETY: `clock` is a readable local.
        let published = unsafe { kinakaze_abi_localtime(&raw const clock) };
        assert!(!published.is_null());
        // SAFETY: as above; the two buffers are separate, so the gmtime result
        // above is still intact.
        assert!(unsafe { (*published).tm_zone }.is_null().eq(&false));

        // `tzset` is callable and leaves the conversions working.
        kinakaze_abi_tzset();
        // SAFETY: `clock` is a readable local.
        assert!(!unsafe { kinakaze_abi_localtime(&raw const clock) }.is_null());
    }

    #[test]
    fn asctime_and_ctime_render_the_documented_form() {
        let tm = utc_tm(REFERENCE);
        // SAFETY: `tm` is a readable local.
        let text = unsafe { kinakaze_abi_asctime(&raw const tm) };
        assert!(!text.is_null());
        // SAFETY: `asctime` returns a null-terminated string in thread-local space.
        let text = unsafe { CStr::from_ptr(text) }
            .to_str()
            .expect("valid UTF-8");
        assert_eq!(text, "Tue Nov 14 22:13:20 2023\n");
        assert_eq!(
            text.len(),
            25,
            "25 bytes plus the terminator is the 26 promised"
        );

        // The day is space-padded, not zero-padded.
        let first = utc_tm(1_609_459_200);
        // SAFETY: `first` is a readable local.
        let text = unsafe { kinakaze_abi_asctime(&raw const first) };
        // SAFETY: as above.
        assert_eq!(
            unsafe { CStr::from_ptr(text) }.to_str(),
            Ok("Fri Jan  1 00:00:00 2021\n")
        );

        // `asctime_r` writes into the caller's 26 bytes.
        let mut buffer = [0i8; ASCTIME_LENGTH];
        // SAFETY: `tm` is readable and the buffer holds the documented 26 bytes.
        let result =
            unsafe { kinakaze_abi_asctime_r(&raw const tm, buffer.as_mut_ptr().cast::<c_char>()) };
        assert!(!result.is_null());
        // SAFETY: the call NUL-terminated the buffer.
        assert_eq!(
            unsafe { CStr::from_ptr(buffer.as_ptr().cast::<c_char>()) }.to_str(),
            Ok("Tue Nov 14 22:13:20 2023\n")
        );

        // `ctime_r` is `asctime_r` of the local time, so it must agree with doing
        // that by hand — which also checks it went through the zone conversion.
        let clock = REFERENCE;
        let mut local = Tm::default();
        // SAFETY: both pointers address writable locals.
        unsafe { kinakaze_abi_localtime_r(&raw const clock, &raw mut local) };
        let mut expected = [0i8; ASCTIME_LENGTH];
        // SAFETY: `local` is readable and the buffer is 26 bytes.
        unsafe { kinakaze_abi_asctime_r(&raw const local, expected.as_mut_ptr().cast::<c_char>()) };
        let mut actual = [0i8; ASCTIME_LENGTH];
        // SAFETY: `clock` is readable and the buffer is 26 bytes.
        let result =
            unsafe { kinakaze_abi_ctime_r(&raw const clock, actual.as_mut_ptr().cast::<c_char>()) };
        assert!(!result.is_null());
        assert_eq!(
            actual, expected,
            "ctime_r must equal asctime_r of localtime_r"
        );

        // A null argument is reported, not dereferenced.
        // SAFETY: passing null is exactly what is being tested.
        assert!(unsafe { kinakaze_abi_asctime(ptr::null()) }.is_null());
    }

    #[test]
    fn difftime_subtracts_exactly() {
        assert_eq!(kinakaze_abi_difftime(REFERENCE, REFERENCE - 60), 60.0);
        assert_eq!(kinakaze_abi_difftime(0, 1), -1.0);
        // A span a double still represents exactly, since it holds 53 integer bits.
        assert_eq!(kinakaze_abi_difftime(1 << 52, 0), (1i64 << 52) as f64);
    }

    #[test]
    fn the_clocks_report_plausible_and_consistent_readings() {
        // Every clock this module claims must be readable.
        for clock in [
            CLOCK_REALTIME,
            CLOCK_MONOTONIC,
            CLOCK_PROCESS_CPUTIME_ID,
            CLOCK_THREAD_CPUTIME_ID,
            CLOCK_MONOTONIC_RAW,
            CLOCK_REALTIME_COARSE,
            CLOCK_MONOTONIC_COARSE,
            CLOCK_BOOTTIME,
        ] {
            let mut value = TimeSpec::default();
            // SAFETY: `value` is a writable local.
            let result = unsafe { kinakaze_abi_clock_gettime(clock, &raw mut value) };
            assert_eq!(result, 0, "clock {clock} should be readable");
            assert!(
                (0..1_000_000_000).contains(&value.tv_nsec),
                "clock {clock} produced an unnormalized timespec"
            );
            assert!(
                value.tv_sec >= 0,
                "clock {clock} produced a negative reading"
            );

            // And its resolution must be a real, positive figure.
            let mut resolution = TimeSpec::default();
            // SAFETY: `resolution` is a writable local.
            let result = unsafe { kinakaze_abi_clock_getres(clock, &raw mut resolution) };
            assert_eq!(result, 0);
            assert!(
                resolution.tv_nsec > 0 || resolution.tv_sec > 0,
                "clock {clock} reported a zero resolution"
            );
        }

        // An unknown clock is EINVAL, not a fabricated reading.
        let mut value = TimeSpec::default();
        // SAFETY: `value` is a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_clock_gettime(99, &raw mut value) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        // SAFETY: as above.
        assert_eq!(unsafe { kinakaze_abi_clock_getres(99, &raw mut value) }, -1);
        // A null timespec is a supported "does this clock exist" probe.
        // SAFETY: passing null is the documented probe form.
        assert_eq!(
            unsafe { kinakaze_abi_clock_getres(CLOCK_REALTIME, ptr::null_mut()) },
            0
        );

        // `time`, `gettimeofday` and `clock_gettime(CLOCK_REALTIME)` describe one
        // clock, so they must agree to within the time the test takes to run.
        // SAFETY: passing null asks for the return value only.
        let seconds = unsafe { kinakaze_abi_time(ptr::null_mut()) };
        let mut through_pointer: Time = 0;
        // SAFETY: `through_pointer` is a writable local.
        let returned = unsafe { kinakaze_abi_time(&raw mut through_pointer) };
        assert_eq!(returned, through_pointer, "both output paths must agree");

        let mut realtime = TimeSpec::default();
        // SAFETY: `realtime` is a writable local.
        unsafe { kinakaze_abi_clock_gettime(CLOCK_REALTIME, &raw mut realtime) };
        assert!(
            (realtime.tv_sec - seconds).abs() <= 2,
            "time() and CLOCK_REALTIME disagree by {} seconds",
            realtime.tv_sec - seconds
        );

        let mut timeval = TimeVal::default();
        // SAFETY: `timeval` is a writable local and a null zone is accepted.
        assert_eq!(
            unsafe { kinakaze_abi_gettimeofday(&raw mut timeval, ptr::null_mut()) },
            0
        );
        assert!((timeval.tv_sec - seconds).abs() <= 2);
        assert!(
            (0..1_000_000).contains(&timeval.tv_usec),
            "tv_usec must be a microsecond"
        );

        // A sanity window on the wall clock. Being outside it would mean the epoch
        // shift is wrong, which is the classic FILETIME conversion mistake.
        assert!(
            (1_700_000_000..4_000_000_000).contains(&seconds),
            "the wall clock reads {seconds}, which is not a plausible Unix time"
        );
    }

    #[test]
    fn the_monotonic_clocks_never_go_backwards() {
        for clock in [
            CLOCK_MONOTONIC,
            CLOCK_MONOTONIC_RAW,
            CLOCK_MONOTONIC_COARSE,
            CLOCK_BOOTTIME,
        ] {
            let mut before = TimeSpec::default();
            let mut after = TimeSpec::default();
            // SAFETY: both are writable locals.
            unsafe { kinakaze_abi_clock_gettime(clock, &raw mut before) };
            std::thread::sleep(Duration::from_millis(20));
            // SAFETY: as above.
            unsafe { kinakaze_abi_clock_gettime(clock, &raw mut after) };

            let elapsed =
                (after.tv_sec - before.tv_sec) * 1_000_000_000 + (after.tv_nsec - before.tv_nsec);
            assert!(elapsed >= 0, "clock {clock} went backwards by {elapsed} ns");
            // The coarse clock's granularity is the system tick, so it is allowed
            // to lag; the others must show most of a 20 ms sleep.
            if clock != CLOCK_MONOTONIC_COARSE {
                assert!(
                    elapsed >= 10_000_000,
                    "clock {clock} advanced only {elapsed} ns across a 20 ms sleep"
                );
            }
        }
    }

    #[test]
    fn times_reports_ticks_at_the_rate_sysconf_promises() {
        let mut buffer = Tms::default();
        // SAFETY: `buffer` is a writable local.
        let elapsed = unsafe { kinakaze_abi_times(&raw mut buffer) };
        assert!(elapsed > 0, "the reference point must be a real tick count");
        assert!(buffer.tms_utime >= 0);
        assert!(buffer.tms_stime >= 0);
        // No child has been reaped, so these are genuinely zero.
        assert_eq!(buffer.tms_cutime, 0);
        assert_eq!(buffer.tms_cstime, 0);

        // The scale must match what `sysconf` tells a caller, or every CPU figure
        // it computes is out by the ratio between them.
        assert_eq!(
            crate::userdb::kinakaze_abi_sysconf(crate::userdb::SC_CLK_TCK),
            CLOCKS_PER_SECOND,
            "times() and sysconf(_SC_CLK_TCK) must agree"
        );

        // A process cannot have used more CPU time than it has existed for. The
        // bound is loose because the reference point is boot, not process start.
        let mut boot = TimeSpec::default();
        // SAFETY: `boot` is a writable local.
        unsafe { kinakaze_abi_clock_gettime(CLOCK_BOOTTIME, &raw mut boot) };
        assert!(
            buffer.tms_utime <= boot.tv_sec * CLOCKS_PER_SECOND + CLOCKS_PER_SECOND,
            "user time exceeds the uptime, so the tick scaling is wrong"
        );

        // A null argument is accepted and only the reference point returned.
        // SAFETY: passing null is the documented form.
        assert!(unsafe { kinakaze_abi_times(ptr::null_mut()) } > 0);
    }

    #[test]
    fn nanosleep_waits_at_least_as_long_as_asked() {
        let requested = TimeSpec {
            tv_sec: 0,
            tv_nsec: 30_000_000,
        };
        let mut remaining = TimeSpec {
            tv_sec: 99,
            tv_nsec: 99,
        };
        let before = monotonic_nanoseconds();
        // SAFETY: both pointers address writable locals.
        let result = unsafe { kinakaze_abi_nanosleep(&raw const requested, &raw mut remaining) };
        let elapsed = monotonic_nanoseconds() - before;

        if result == 0 {
            assert!(
                elapsed >= 30_000_000,
                "an uninterrupted sleep returned after only {elapsed} ns"
            );
        } else {
            // A signal from elsewhere in the test binary cut it short, which is a
            // correct outcome; what must hold is the remaining-time arithmetic.
            assert_eq!(kinakaze_tls::errno(), EINTR);
            let left = remaining.tv_sec * 1_000_000_000 + remaining.tv_nsec;
            assert!(
                (0..=30_000_000).contains(&left),
                "the remaining time {left} is not within the interval requested"
            );
            assert!((0..1_000_000_000).contains(&remaining.tv_nsec));
        }

        // Invalid intervals are refused rather than slept through.
        for invalid in [
            TimeSpec {
                tv_sec: -1,
                tv_nsec: 0,
            },
            TimeSpec {
                tv_sec: 0,
                tv_nsec: 1_000_000_000,
            },
            TimeSpec {
                tv_sec: 0,
                tv_nsec: -1,
            },
        ] {
            // SAFETY: `invalid` is a readable local and a null remainder is allowed.
            let result = unsafe { kinakaze_abi_nanosleep(&raw const invalid, ptr::null_mut()) };
            assert_eq!(result, -1);
            assert_eq!(kinakaze_tls::errno(), EINVAL);
        }
        // SAFETY: passing null is exactly what is being tested.
        assert_eq!(
            unsafe { kinakaze_abi_nanosleep(ptr::null(), ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EFAULT);
    }

    #[test]
    fn clock_nanosleep_honours_both_of_its_modes() {
        let relative = TimeSpec {
            tv_sec: 0,
            tv_nsec: 20_000_000,
        };
        let before = monotonic_nanoseconds();
        // SAFETY: `relative` is a readable local and a null remainder is allowed.
        let result = unsafe {
            kinakaze_abi_clock_nanosleep(CLOCK_MONOTONIC, 0, &raw const relative, ptr::null_mut())
        };
        // The error is the return value here, not errno; 0 or EINTR are both valid.
        assert!(result == 0 || result == EINTR, "unexpected result {result}");
        if result == 0 {
            assert!(monotonic_nanoseconds() - before >= 20_000_000);
        }

        // An absolute deadline already in the past returns at once.
        let mut now = TimeSpec::default();
        // SAFETY: `now` is a writable local.
        unsafe { kinakaze_abi_clock_gettime(CLOCK_MONOTONIC, &raw mut now) };
        let past = TimeSpec {
            tv_sec: now.tv_sec - 10,
            tv_nsec: now.tv_nsec,
        };
        let before = monotonic_nanoseconds();
        // SAFETY: `past` is a readable local.
        let result = unsafe {
            kinakaze_abi_clock_nanosleep(
                CLOCK_MONOTONIC,
                TIMER_ABSTIME,
                &raw const past,
                ptr::null_mut(),
            )
        };
        assert_eq!(result, 0);
        assert!(
            monotonic_nanoseconds() - before < 20_000_000,
            "a past deadline should not have waited"
        );

        // The CPU-time clocks cannot be slept against on either platform.
        // SAFETY: `relative` is a readable local.
        assert_eq!(
            unsafe {
                kinakaze_abi_clock_nanosleep(
                    CLOCK_THREAD_CPUTIME_ID,
                    0,
                    &raw const relative,
                    ptr::null_mut(),
                )
            },
            EINVAL
        );
        // Errors come back through the return value, so a bad interval does too.
        let invalid = TimeSpec {
            tv_sec: 0,
            tv_nsec: -1,
        };
        // SAFETY: `invalid` is a readable local.
        assert_eq!(
            unsafe {
                kinakaze_abi_clock_nanosleep(
                    CLOCK_MONOTONIC,
                    0,
                    &raw const invalid,
                    ptr::null_mut(),
                )
            },
            EINVAL
        );
    }

    #[test]
    fn alarm_and_setitimer_share_one_timer() {
        // The timer is armed far enough out that it never fires during the test.
        // Letting it fire would raise a real SIGALRM, whose default action
        // terminates the process — which here is the test runner.
        assert_eq!(
            kinakaze_abi_alarm(0),
            0,
            "no alarm is pending to begin with"
        );

        assert_eq!(
            kinakaze_abi_alarm(600),
            0,
            "arming reports the previous alarm"
        );
        let remaining = kinakaze_abi_alarm(600);
        assert!(
            (598..=600).contains(&remaining),
            "the previous alarm had {remaining} seconds left, not about 600"
        );

        // `setitimer(ITIMER_REAL)` is the same timer, so it must see the alarm.
        let mut previous = ItimerVal::default();
        let disarm = ItimerVal::default();
        // SAFETY: both pointers address locals of the right type.
        let result =
            unsafe { kinakaze_abi_setitimer(ITIMER_REAL, &raw const disarm, &raw mut previous) };
        assert_eq!(result, 0);
        assert!(
            (598..=600).contains(&previous.it_value.tv_sec),
            "setitimer reported {} seconds left, so it is not the same timer as alarm",
            previous.it_value.tv_sec
        );
        // And the disarm took effect.
        assert_eq!(kinakaze_abi_alarm(0), 0, "the timer should now be disarmed");

        // A periodic timer's interval is reported back.
        let periodic = ItimerVal {
            it_interval: TimeVal {
                tv_sec: 300,
                tv_usec: 0,
            },
            it_value: TimeVal {
                tv_sec: 600,
                tv_usec: 500_000,
            },
        };
        // SAFETY: `periodic` is a readable local; a null old value is allowed.
        assert_eq!(
            unsafe { kinakaze_abi_setitimer(ITIMER_REAL, &raw const periodic, ptr::null_mut()) },
            0
        );
        let mut previous = ItimerVal::default();
        // SAFETY: both pointers address locals of the right type.
        unsafe { kinakaze_abi_setitimer(ITIMER_REAL, &raw const disarm, &raw mut previous) };
        assert_eq!(
            previous.it_interval.tv_sec, 300,
            "the interval must survive"
        );
        assert!((598..=601).contains(&previous.it_value.tv_sec));

        // Leave nothing armed for the rest of the suite.
        assert_eq!(kinakaze_abi_alarm(0), 0);

        // The CPU-time timers are refused rather than silently ignored.
        for which in [ITIMER_VIRTUAL, ITIMER_PROF] {
            // SAFETY: `disarm` is a readable local.
            let result =
                unsafe { kinakaze_abi_setitimer(which, &raw const disarm, ptr::null_mut()) };
            assert_eq!(result, -1);
            assert_eq!(
                kinakaze_tls::errno(),
                ENOSYS,
                "timer {which} should report ENOSYS"
            );
        }
        // An unknown timer kind, and a malformed interval.
        // SAFETY: `disarm` is a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_setitimer(99, &raw const disarm, ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        let malformed = ItimerVal {
            it_interval: TimeVal::default(),
            it_value: TimeVal {
                tv_sec: 0,
                tv_usec: 1_000_000,
            },
        };
        // SAFETY: `malformed` is a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_setitimer(ITIMER_REAL, &raw const malformed, ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        // SAFETY: passing null is exactly what is being tested.
        assert_eq!(
            unsafe { kinakaze_abi_setitimer(ITIMER_REAL, ptr::null(), ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EFAULT);
    }

    #[test]
    fn setting_the_clock_reports_refusals_and_never_pretends() {
        // Nothing here reaches `SetSystemTime`. Every case is one this module
        // rejects before calling the host, which is deliberate: a test that
        // actually stepped the wall clock would change the machine's time, and on
        // an elevated runner it would succeed at doing so.

        // A clock other than CLOCK_REALTIME cannot be set on Linux either.
        let value = TimeSpec {
            tv_sec: 1,
            tv_nsec: 0,
        };
        // SAFETY: `value` is a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_clock_settime(CLOCK_MONOTONIC, &raw const value) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);

        // A malformed timespec is refused before any host call.
        let malformed = TimeSpec {
            tv_sec: 1,
            tv_nsec: 1_000_000_000,
        };
        // SAFETY: `malformed` is a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_clock_settime(CLOCK_REALTIME, &raw const malformed) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        // SAFETY: passing null is exactly what is being tested.
        assert_eq!(
            unsafe { kinakaze_abi_clock_settime(CLOCK_REALTIME, ptr::null()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EFAULT);

        // `settimeofday` likewise.
        let malformed = TimeVal {
            tv_sec: 1,
            tv_usec: 1_000_000,
        };
        // SAFETY: `malformed` is a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_settimeofday(&raw const malformed, ptr::null()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        // A null value with a zone argument sets nothing and succeeds, as on Linux.
        // SAFETY: passing null is the documented form.
        assert_eq!(
            unsafe { kinakaze_abi_settimeofday(ptr::null(), ptr::null()) },
            0
        );

        // An instant with no `SYSTEMTIME` representation is refused rather than
        // truncated into a different year.
        let unrepresentable = TimeSpec {
            // Well before 1601, the FILETIME epoch.
            tv_sec: -20_000_000_000,
            tv_nsec: 0,
        };
        // SAFETY: `unrepresentable` is a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_clock_settime(CLOCK_REALTIME, &raw const unrepresentable) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn adjtimex_reads_real_figures_and_refuses_what_it_cannot_do() {
        // Read-only mode: `modes` of 0 changes nothing, so this is safe to run.
        let mut buffer = Timex::default();
        // SAFETY: `buffer` is a writable local.
        let state = unsafe { kinakaze_abi_adjtimex(&raw mut buffer) };
        // STA_UNSYNC is reported because no synchronization loop exists here, and
        // TIME_ERROR is the state code that goes with it.
        assert_eq!(state, TIME_ERROR);
        assert_eq!(buffer.status & STA_UNSYNC, STA_UNSYNC);
        // `tick` is the host's real per-interrupt increment in microseconds. A
        // Windows tick is between about 0.5 ms and 16 ms.
        assert!(
            (500..=20_000).contains(&buffer.tick),
            "tick of {} microseconds is not a plausible timer increment",
            buffer.tick
        );
        // The time field is the wall clock, so it must agree with `time`.
        // SAFETY: passing null asks for the return value only.
        let seconds = unsafe { kinakaze_abi_time(ptr::null_mut()) };
        assert!((buffer.time.tv_sec - seconds).abs() <= 2);
        assert!((0..1_000_000).contains(&buffer.time.tv_usec));
        // The fields this module cannot model are zero rather than invented.
        assert_eq!(buffer.maxerror, 0);
        assert_eq!(buffer.esterror, 0);
        assert_eq!(buffer.constant, 0);
        assert_eq!(buffer.jitter, 0);

        // A mode naming a phase-locked loop is refused, and nothing is applied.
        let mut request = Timex {
            modes: ADJ_OFFSET,
            offset: 1000,
            ..Timex::default()
        };
        // SAFETY: `request` is a writable local.
        assert_eq!(unsafe { kinakaze_abi_adjtimex(&raw mut request) }, -1);
        assert_eq!(kinakaze_tls::errno(), ENOSYS);

        // A mode bit Linux does not define is EINVAL.
        let mut request = Timex {
            modes: 0x4000_0000,
            ..Timex::default()
        };
        // SAFETY: `request` is a writable local.
        assert_eq!(unsafe { kinakaze_abi_adjtimex(&raw mut request) }, -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);

        // SAFETY: passing null is exactly what is being tested.
        assert_eq!(unsafe { kinakaze_abi_adjtimex(ptr::null_mut()) }, -1);
        assert_eq!(kinakaze_tls::errno(), EFAULT);
    }

    #[test]
    fn civil_date_arithmetic_is_exact_across_the_awkward_cases() {
        // Known day numbers, verified against the calendar rather than each other.
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2024, 2, 29), 19_782);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(days_from_civil(1600, 1, 1), -135_140);

        // The two directions must be mutual inverses over a wide span, including
        // every century and 400-year boundary in it.
        for day in (-200_000..200_000).step_by(97) {
            let (year, month, mday) = civil_from_days(day);
            assert_eq!(
                days_from_civil(year, month, mday),
                day,
                "day {day} did not survive"
            );
            assert!(
                (1..=12).contains(&month),
                "day {day} produced month {month}"
            );
            assert!(
                (1..=days_in_month(year, month)).contains(&mday),
                "day {day} produced {year}-{month}-{mday}"
            );
        }

        // The leap-year rules, including the century exception and its exception.
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2023));
        assert!(!is_leap_year(1900), "a century year is not a leap year");
        assert!(is_leap_year(2000), "unless it is divisible by 400");
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2023, 2), 28);

        // The nth-weekday rule both transition forms rely on. March 2023 began on
        // a Wednesday, so the second Sunday is the 12th and the last is the 26th.
        assert_eq!(nth_weekday_of_month(2023, 3, 2, 0), 12);
        assert_eq!(
            nth_weekday_of_month(2023, 3, 5, 0),
            26,
            "week 5 means the last"
        );
        assert_eq!(nth_weekday_of_month(2023, 3, 1, 0), 5);
        // November 2023 began on a Wednesday, so the first Sunday is the 5th.
        assert_eq!(nth_weekday_of_month(2023, 11, 1, 0), 5);
        // February 2023 had exactly four Wednesdays, so week 5 falls back to the
        // fourth rather than running past the end of the month.
        assert_eq!(nth_weekday_of_month(2023, 2, 5, 3), 22);
    }

    #[test]
    fn the_zone_abbreviation_falls_back_to_a_numeric_form() {
        // Windows names a zone "Eastern Standard Time", which is not an
        // abbreviation, so the numeric form is used instead. It has to be the ISO
        // spelling a caller parsing `%Z` can make sense of.
        assert_eq!(ZoneAbbrev::numeric(0).as_str(), "+00");
        assert_eq!(ZoneAbbrev::numeric(32_400).as_str(), "+09");
        assert_eq!(ZoneAbbrev::numeric(-18_000).as_str(), "-05");
        assert_eq!(ZoneAbbrev::numeric(19_800).as_str(), "+0530");
        assert_eq!(ZoneAbbrev::numeric(-16_200).as_str(), "-0430");

        // An interned name is stable, so a guest may hold `tm_zone` indefinitely.
        let first = published_abbrev(ZoneAbbrev::new("UTC"));
        let second = published_abbrev(ZoneAbbrev::new("UTC"));
        assert_eq!(first, second, "one name must intern to one pointer");
        // SAFETY: the pointer addresses a leaked CString that lives for the process.
        assert_eq!(unsafe { CStr::from_ptr(first) }.to_str(), Ok("UTC"));

        // `%Z` on a struct the guest built itself, with no `tm_zone`, describes the
        // offset the struct actually carries rather than guessing at a name.
        let tm = Tm {
            tm_gmtoff: 19_800,
            tm_zone: ptr::null(),
            ..Tm::default()
        };
        assert_eq!(format(&tm, "%Z"), "+0530");
        assert_eq!(format(&tm, "%z"), "+0530");
        // And a negative offset keeps its sign in both conversions.
        let tm = Tm {
            tm_gmtoff: -18_000,
            tm_zone: ptr::null(),
            ..Tm::default()
        };
        assert_eq!(format(&tm, "%z"), "-0500");
        assert_eq!(format(&tm, "%Z"), "-05");
    }
}

/// The Linux LP64 timeb layout, including its historical unused timezone fields.
#[repr(C)]
pub struct Timeb {
    time: i64,
    millitm: u16,
    timezone: i16,
    dstflag: i16,
}

/// # Safety
/// `value` must point to writable `struct timeb` storage.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ftime(value: *mut Timeb) -> c_int {
    if value.is_null() {
        crate::set_errno(14);
        return -1;
    }
    let (seconds, nanoseconds) = read_realtime(true);
    unsafe {
        value.write(Timeb {
            time: seconds,
            millitm: (nanoseconds / 1_000_000) as u16,
            timezone: 0,
            dstflag: 0,
        });
    }
    0
}

fn thread_clock(clock: i32) -> Result<(i64, i64), i32> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, GetProcessIdOfThread, GetThreadTimes, OpenThread,
        THREAD_QUERY_LIMITED_INFORMATION,
    };
    let tid = !(clock >> 3) as u32;
    let handle = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, tid) };
    if handle.is_null() {
        return Err(EINVAL);
    }
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let success = unsafe {
        GetProcessIdOfThread(handle) == GetCurrentProcessId()
            && GetThreadTimes(
                handle,
                &raw mut created,
                &raw mut exited,
                &raw mut kernel,
                &raw mut user,
            ) != 0
    };
    unsafe {
        CloseHandle(handle);
    }
    if !success {
        return Err(EINVAL);
    }
    let ticks = ((kernel.dwHighDateTime as u64) << 32 | kernel.dwLowDateTime as u64)
        + ((user.dwHighDateTime as u64) << 32 | user.dwLowDateTime as u64);
    Ok((
        (ticks / 10_000_000) as i64,
        ((ticks % 10_000_000) * 100) as i64,
    ))
}
