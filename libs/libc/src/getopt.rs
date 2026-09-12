//! The `getopt` family: short options, GNU long options, and argv permutation.
//!
//! BusyBox routes every applet's command line through these entry points, so
//! the GNU behaviours matter as much as the POSIX ones: `argv` is permuted so
//! that non-options collect at the end, long options may be abbreviated, and
//! `optind = 0` requests a fresh scan.
//!
//! `optind`, `optopt`, `opterr` and `optarg` are *data* exports. A guest reads
//! and writes them directly, so each is a `#[repr(transparent)]` wrapper around
//! an atomic: the storage lands in a writable section, a guest assigning to one
//! is not a data race, and the layout stays the bare `int` or `char *` the guest
//! expects.
//!
//! The scan position inside a cluster like `-abc` cannot live in `optind`, which
//! still addresses the cluster itself, so it is held in [`STATE`] alongside the
//! bounds of the skipped non-option run that permutation rotates.

use core::ffi::{c_char, c_int};
use core::ptr;
use std::sync::{Mutex, MutexGuard};

/// `has_arg`: the option takes no argument.
pub const NO_ARGUMENT: c_int = 0;
/// `has_arg`: the option requires an argument.
pub const REQUIRED_ARGUMENT: c_int = 1;
/// `has_arg`: the option takes an optional argument.
pub const OPTIONAL_ARGUMENT: c_int = 2;

/// `struct option`, the long-option table entry.
///
/// A null `name` terminates the table. When `flag` is non-null `getopt_long`
/// stores `val` through it and returns 0 instead of returning `val`.
#[repr(C)]
pub struct LongOption {
    pub name: *const c_char,
    pub has_arg: c_int,
    pub flag: *mut c_int,
    pub val: c_int,
}

// The four exported variables are `crate::copied` types rather than plain atomics.
//
// A guest that references a libc variable gets an `R_X86_64_COPY` relocation and
// therefore its *own* storage; on Linux, libc's internal accesses are redirected to
// that copy through its GOT, but a Windows PE's direct accesses cannot be. Without an
// explicit redirect there are two `optind`s, and only the guest's is authoritative —
// `busybox ls` parsed `-l` correctly and then listed a file called `ls`, because this
// libc advanced its own copy while BusyBox read its own. See [`crate::copied`].
use crate::copied::{CopiedInt, CopiedPointer};

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
/// `optind`: the index of the next `argv` element to examine.
///
/// Starts at 1, as glibc's does, because `argv[0]` is the program name. Setting
/// it to 0 requests a reinitialised scan.
pub static kinakaze_abi_optind: CopiedInt = CopiedInt::new(1);

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
pub static optind: CopiedInt = CopiedInt::new(1);

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
/// `optopt`: the option character that provoked the last error.
pub static kinakaze_abi_optopt: CopiedInt = CopiedInt::new(0);

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
pub static optopt: CopiedInt = CopiedInt::new(0);

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
/// `opterr`: when nonzero, errors are reported on stderr.
pub static kinakaze_abi_opterr: CopiedInt = CopiedInt::new(1);

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
pub static opterr: CopiedInt = CopiedInt::new(1);

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
/// `optarg`: the argument of the option just returned, or null.
pub static kinakaze_abi_optarg: CopiedPointer = CopiedPointer::new();

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
pub static optarg: CopiedPointer = CopiedPointer::new();

/// How non-option arguments are treated.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ordering {
    /// Collect non-options at the end of `argv`; the GNU default.
    Permute,
    /// Stop at the first non-option, requested by a leading `+`.
    RequireOrder,
    /// Return non-options as character 1, requested by a leading `-`.
    ReturnInOrder,
}

/// Scan state that has nowhere to live in the exported variables.
struct State {
    /// The element being scanned and the offset of the next character in it,
    /// which is how a cluster such as `-abc` is resumed across calls.
    nextchar: Option<(c_int, usize)>,
    ordering: Ordering,
    /// Bounds of the run of skipped non-options, `[first, last)`.
    first_nonopt: c_int,
    last_nonopt: c_int,
    /// Cleared by a reset so the next call re-reads the ordering prefix.
    initialised: bool,
}

impl State {
    const fn new() -> Self {
        Self {
            nextchar: None,
            ordering: Ordering::Permute,
            first_nonopt: 1,
            last_nonopt: 1,
            initialised: false,
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());

fn state() -> MutexGuard<'static, State> {
    // A poisoned lock would only mean a previous caller panicked mid-scan; the
    // state is plain integers, so recovering it is preferable to aborting the
    // guest.
    STATE.lock().unwrap_or_else(|error| error.into_inner())
}

/// Borrows a null-terminated string as bytes, excluding the terminator.
///
/// # Safety
///
/// `text` must be a non-null pointer to a null-terminated string that outlives
/// the returned slice.
unsafe fn bytes<'a>(text: *const c_char) -> &'a [u8] {
    // SAFETY: the caller guarantees a terminator, so the length is finite.
    let length = unsafe { crate::string::strlen(text) };
    // SAFETY: `length` readable bytes precede the terminator.
    unsafe { core::slice::from_raw_parts(text.cast::<u8>(), length) }
}

/// Reads `argv[index]`.
///
/// # Safety
///
/// `index` must be less than the `argc` the caller passed in.
unsafe fn arg(argv: *const *mut c_char, index: c_int) -> *mut c_char {
    // SAFETY: the caller keeps `index` inside the array.
    unsafe { *argv.add(index as usize) }
}

/// Reports a diagnostic on stderr, in glibc's `program: message` shape.
///
/// Writes straight to descriptor 2 rather than through a `FILE`, so a guest that
/// redirected stderr still sees it and no stdio buffer has to be flushed.
fn report(argv0: &[u8], parts: &[&[u8]]) {
    let mut line = Vec::new();
    line.extend_from_slice(argv0);
    line.extend_from_slice(b": ");
    for part in parts {
        line.extend_from_slice(part);
    }
    line.push(b'\n');
    // A failed diagnostic write has nowhere left to be reported.
    let _ = kinakaze_vfs::write(2, &line);
}

/// Rotates the skipped non-options behind the options found after them.
///
/// glibc calls this `exchange`. The two adjacent runs are
/// `[first_nonopt, last_nonopt)` of non-options and `[last_nonopt, optind)` of
/// options; swapping them is a rotation, done as three reversals so no
/// scratch allocation is needed.
///
/// # Safety
///
/// The recorded bounds must be within `argv`.
unsafe fn exchange(argv: *mut *mut c_char, scan: &mut State) {
    let first = scan.first_nonopt;
    let middle = scan.last_nonopt;
    let last = kinakaze_abi_optind.get();
    if first >= middle || middle >= last {
        // Nothing to rotate; just re-anchor the run on the new position.
        scan.first_nonopt = kinakaze_abi_optind.get();
        scan.last_nonopt = kinakaze_abi_optind.get();
        return;
    }

    // SAFETY: the bounds lie inside `argv`, so every index reversed is valid.
    unsafe {
        reverse(argv, first, middle);
        reverse(argv, middle, last);
        reverse(argv, first, last);
    }

    // The options now sit where the non-options were, so the skipped run starts
    // that many elements later and ends where the scan has reached.
    scan.first_nonopt += last - middle;
    scan.last_nonopt = last;
}

/// Reverses `argv[from..to]`.
///
/// # Safety
///
/// `from` and `to` must bound a range inside `argv`.
unsafe fn reverse(argv: *mut *mut c_char, from: c_int, to: c_int) {
    let mut low = from as usize;
    let mut high = (to as usize).saturating_sub(1);
    while low < high {
        // SAFETY: both indices remain inside the array.
        unsafe { ptr::swap(argv.add(low), argv.add(high)) };
        low += 1;
        high -= 1;
    }
}

/// Returns true when `POSIXLY_CORRECT` is present in the environment.
fn posixly_correct() -> bool {
    const NAME: &[u8] = b"POSIXLY_CORRECT\0";
    // SAFETY: the name is a null-terminated literal.
    let value = unsafe { crate::process::kinakaze_abi_getenv(NAME.as_ptr().cast::<c_char>()) };
    !value.is_null()
}

/// Reinitialises the scan and reads the ordering prefix of `optstring`.
///
/// Returns `optstring` with any leading `-` or `+` removed. A leading `:` is
/// left in place: it only selects how a missing argument is reported, and the
/// option-character search skips `:` anyway.
fn initialise(optstring: &[u8], scan: &mut State) -> Ordering {
    if kinakaze_abi_optind.get() == 0 {
        kinakaze_abi_optind.set(1);
    }
    scan.first_nonopt = kinakaze_abi_optind.get();
    scan.last_nonopt = kinakaze_abi_optind.get();
    scan.nextchar = None;
    scan.initialised = true;

    match optstring.first() {
        Some(b'-') => Ordering::ReturnInOrder,
        Some(b'+') => Ordering::RequireOrder,
        _ if posixly_correct() => Ordering::RequireOrder,
        _ => Ordering::Permute,
    }
}

/// True when `argv[index]` is not an option: it does not begin with `-`, or it
/// is the single character `-`.
///
/// # Safety
///
/// `index` must be a valid index into `argv`.
unsafe fn is_nonoption(argv: *const *mut c_char, index: c_int) -> bool {
    // SAFETY: the caller keeps `index` in range.
    let text = unsafe { arg(argv, index) };
    if text.is_null() {
        return true;
    }
    // SAFETY: `argv` entries are null-terminated strings.
    let text = unsafe { bytes(text) };
    text.first() != Some(&b'-') || text.len() == 1
}

/// The outcome of positioning `optind` on the next thing to look at.
enum Advance {
    /// No options remain.
    Done,
    /// A non-option, to be returned as character 1 under `ReturnInOrder`.
    InOrder,
    /// `optind` addresses an option to decode.
    Found,
}

/// Moves `optind` onto the next option, permuting non-options out of the way.
///
/// # Safety
///
/// `argv` must hold `argc` entries.
unsafe fn advance(argc: c_int, argv: *mut *mut c_char, scan: &mut State) -> Advance {
    // Non-options already skipped must stay adjacent to any further ones, so a
    // run interrupted by the scan is rotated back together first.
    if scan.last_nonopt > kinakaze_abi_optind.get() {
        scan.last_nonopt = kinakaze_abi_optind.get();
    }
    if scan.first_nonopt > kinakaze_abi_optind.get() {
        scan.first_nonopt = kinakaze_abi_optind.get();
    }

    if scan.ordering == Ordering::Permute {
        // Rotate a run of non-options found before this point behind the
        // options that followed them.
        if scan.first_nonopt != scan.last_nonopt && scan.last_nonopt != kinakaze_abi_optind.get() {
            // SAFETY: the bounds lie inside `argv`.
            unsafe { exchange(argv, scan) };
        } else if scan.last_nonopt != kinakaze_abi_optind.get() {
            scan.first_nonopt = kinakaze_abi_optind.get();
        }

        // Skip the non-options; permutation will move them to the end.
        // SAFETY: the index is bounded by `argc`.
        while kinakaze_abi_optind.get() < argc
            && unsafe { is_nonoption(argv, kinakaze_abi_optind.get()) }
        {
            kinakaze_abi_optind.set(kinakaze_abi_optind.get() + 1);
        }
        scan.last_nonopt = kinakaze_abi_optind.get();
    }

    // `--` ends the options. `optind` must finish past it, which the rotation
    // below arranges by treating the delimiter as one of the options.
    if kinakaze_abi_optind.get() != argc && {
        // SAFETY: `optind` is in range and entries are null-terminated.
        let text = unsafe { arg(argv, kinakaze_abi_optind.get()) };
        !text.is_null() && unsafe { bytes(text) } == b"--"
    } {
        kinakaze_abi_optind.set(kinakaze_abi_optind.get() + 1);

        if scan.first_nonopt != scan.last_nonopt && scan.last_nonopt != kinakaze_abi_optind.get() {
            // SAFETY: the recorded bounds lie inside `argv`.
            unsafe { exchange(argv, scan) };
        } else if scan.first_nonopt == scan.last_nonopt {
            scan.first_nonopt = kinakaze_abi_optind.get();
        }
        scan.last_nonopt = argc;
        kinakaze_abi_optind.set(argc);
    }

    // At the end of `argv`, leave `optind` at the first non-option so the caller
    // can pick the operands up from there.
    if kinakaze_abi_optind.get() == argc {
        if scan.first_nonopt != scan.last_nonopt {
            kinakaze_abi_optind.set(scan.first_nonopt);
        }
        return Advance::Done;
    }

    // Only `RequireOrder` and `ReturnInOrder` can still be sitting on a
    // non-option: `Permute` skipped them all above.
    // SAFETY: `optind` is in range.
    if unsafe { is_nonoption(argv, kinakaze_abi_optind.get()) } {
        if scan.ordering == Ordering::RequireOrder {
            return Advance::Done;
        }
        return Advance::InOrder;
    }

    Advance::Found
}

/// Locates an option character in the (prefix-stripped) `optstring`.
///
/// `:` never names an option, only an argument marker, so it is rejected before
/// the search: otherwise `-:` would match the separator of an earlier option.
fn find_short(optstring: &[u8], character: u8) -> Option<usize> {
    if character == b':' {
        return None;
    }
    optstring.iter().position(|byte| *byte == character)
}

/// Everything the long-option matcher needs that is not scan state.
struct LongScan<'a> {
    argc: c_int,
    argv: *mut *mut c_char,
    /// `optstring` with any ordering prefix removed.
    optstring: &'a [u8],
    longopts: *const LongOption,
    longindex: *mut c_int,
    /// Report diagnostics on stderr.
    print_errors: bool,
    /// `optstring` began with `:`, so a missing argument returns `:`.
    colon: bool,
    /// A single dash may introduce a long option.
    long_only: bool,
}

/// Attempts to read `argv[optind]` as a long option.
///
/// Returns `None` only in the `getopt_long_only` case where the word matched no
/// long option but its first character is a valid short option, which the caller
/// then decodes as a short cluster.
///
/// # Safety
///
/// `argv` must hold `argc` entries and `longopts` must be a table terminated by
/// a null `name`.
unsafe fn try_long(scan: &mut State, context: &LongScan<'_>, offset: usize) -> Option<c_int> {
    let index = kinakaze_abi_optind.get();
    // SAFETY: `optind` is in range and entries are null-terminated.
    let whole = unsafe { bytes(arg(context.argv, index)) };
    let name = &whole[offset..];
    // `--name=value` splits at the first `=`.
    let split = name.iter().position(|byte| *byte == b'=');
    let wanted = match split {
        Some(at) => &name[..at],
        None => name,
    };
    // The dashes as written, so diagnostics echo `-foo` or `--foo` faithfully.
    let prefix = &whole[..offset];

    let mut found: Option<(usize, &LongOption)> = None;
    let mut exact = false;
    let mut ambiguous = false;
    let mut cursor = 0;
    loop {
        // SAFETY: the table is terminated by an entry with a null `name`.
        let entry = unsafe { &*context.longopts.add(cursor) };
        if entry.name.is_null() {
            break;
        }
        // SAFETY: a non-null table name is a null-terminated string.
        let candidate = unsafe { bytes(entry.name) };
        if candidate.starts_with(wanted) {
            if candidate.len() == wanted.len() {
                found = Some((cursor, entry));
                exact = true;
                break;
            }
            match found {
                None => found = Some((cursor, entry)),
                // Two different entries share the prefix. Aliases that agree in
                // every field are not a conflict, which is how glibc lets a
                // table list one option twice.
                Some((_, previous)) => {
                    if context.long_only
                        || previous.has_arg != entry.has_arg
                        || previous.flag != entry.flag
                        || previous.val != entry.val
                    {
                        ambiguous = true;
                    }
                }
            }
        }
        cursor += 1;
    }

    if ambiguous && !exact {
        if context.print_errors {
            // SAFETY: `argv[0]` is a null-terminated string.
            let argv0 = unsafe { bytes(arg(context.argv, 0)) };
            report(argv0, &[b"option '", whole, b"' is ambiguous"]);
        }
        scan.nextchar = None;
        kinakaze_abi_optind.set(index + 1);
        kinakaze_abi_optopt.set(0);
        return Some(i32::from(b'?'));
    }

    let Some((position, entry)) = found else {
        // Not a long option. Under `getopt_long_only` a single dash whose first
        // character is a known short option is a short cluster after all.
        if !context.long_only || whole.get(1) == Some(&b'-') || {
            let first = name.first().copied().unwrap_or(0);
            find_short(context.optstring, first).is_none()
        } {
            if context.print_errors {
                // SAFETY: `argv[0]` is null-terminated.
                let argv0 = unsafe { bytes(arg(context.argv, 0)) };
                report(argv0, &[b"unrecognized option '", prefix, name, b"'"]);
            }
            scan.nextchar = None;
            kinakaze_abi_optind.set(index + 1);
            kinakaze_abi_optopt.set(0);
            return Some(i32::from(b'?'));
        }
        return None;
    };

    kinakaze_abi_optind.set(index + 1);
    scan.nextchar = None;

    // SAFETY: the matched entry's name is null-terminated.
    let matched = unsafe { bytes(entry.name) };
    if let Some(at) = split {
        if entry.has_arg == NO_ARGUMENT {
            if context.print_errors {
                // SAFETY: `argv[0]` is null-terminated.
                let argv0 = unsafe { bytes(arg(context.argv, 0)) };
                report(
                    argv0,
                    &[b"option '", prefix, matched, b"' doesn't allow an argument"],
                );
            }
            kinakaze_abi_optopt.set(entry.val);
            return Some(i32::from(b'?'));
        }
        // The argument is the tail of this same element, after the `=`.
        // SAFETY: `at` indexes within the element, so `offset + at + 1` is at
        // worst the terminator.
        let value = unsafe { arg(context.argv, index).add(offset + at + 1) };
        kinakaze_abi_optarg.set(value);
    } else if entry.has_arg == REQUIRED_ARGUMENT {
        if kinakaze_abi_optind.get() >= context.argc {
            if context.print_errors {
                // SAFETY: `argv[0]` is null-terminated.
                let argv0 = unsafe { bytes(arg(context.argv, 0)) };
                report(
                    argv0,
                    &[b"option '", prefix, matched, b"' requires an argument"],
                );
            }
            kinakaze_abi_optopt.set(entry.val);
            return Some(i32::from(if context.colon { b':' } else { b'?' }));
        }
        // SAFETY: the index was just bounds-checked against `argc`.
        let value = unsafe { arg(context.argv, kinakaze_abi_optind.get()) };
        kinakaze_abi_optarg.set(value);
        kinakaze_abi_optind.set(kinakaze_abi_optind.get() + 1);
    }

    if !context.longindex.is_null() {
        // SAFETY: the caller supplies a writable `int` or null.
        unsafe { *context.longindex = position as c_int };
    }
    if !entry.flag.is_null() {
        // SAFETY: a non-null `flag` points to a writable `int`.
        unsafe { *entry.flag = entry.val };
        return Some(0);
    }
    Some(entry.val)
}

/// The shared body of `getopt`, `getopt_long` and `getopt_long_only`.
///
/// # Safety
///
/// `argv` must hold `argc` entries of null-terminated strings, `optstring` must
/// be null-terminated, and `longopts`, when non-null, must be terminated by an
/// entry with a null `name`.
unsafe fn getopt_internal(
    argc: c_int,
    argv: *mut *mut c_char,
    optstring: *const c_char,
    longopts: *const LongOption,
    longindex: *mut c_int,
    long_only: bool,
) -> c_int {
    if argc < 1 || argv.is_null() || optstring.is_null() {
        return -1;
    }

    // Each call reports only its own option's argument.
    kinakaze_abi_optarg.set(ptr::null_mut());

    // SAFETY: the caller guarantees a null-terminated string.
    let full = unsafe { bytes(optstring) };
    let scan = &mut *state();

    // A guest requests a fresh scan with `optind = 0`; the first call ever also
    // has to initialise.
    if kinakaze_abi_optind.get() == 0 || !scan.initialised {
        scan.ordering = initialise(full, scan);
    }

    // The ordering prefix is not part of the option characters. A leading `:`
    // stays: it selects the missing-argument return value.
    let stripped = match full.first() {
        Some(b'-' | b'+') => &full[1..],
        _ => full,
    };
    let colon = stripped.first() == Some(&b':');
    // `opterr` is consulted per call, so a guest may silence errors at any time.
    let print_errors = kinakaze_abi_opterr.get() != 0 && !colon;

    // Resume a cluster such as `-abc`, but only within the element it started
    // in: anything else means the scan has moved on.
    let resume = match scan.nextchar {
        Some((index, offset)) if index == kinakaze_abi_optind.get() => Some(offset),
        _ => {
            scan.nextchar = None;
            None
        }
    };

    let offset = match resume {
        Some(offset) => offset,
        None => {
            match unsafe { advance(argc, argv, scan) } {
                Advance::Done => return -1,
                Advance::InOrder => {
                    // `-` ordering hands the operand back as character 1.
                    // SAFETY: `optind` is in range.
                    kinakaze_abi_optarg.set(unsafe { arg(argv, kinakaze_abi_optind.get()) });
                    kinakaze_abi_optind.set(kinakaze_abi_optind.get() + 1);
                    return 1;
                }
                Advance::Found => {}
            }

            // SAFETY: `optind` is in range and entries are null-terminated.
            let text = unsafe { bytes(arg(argv, kinakaze_abi_optind.get())) };
            // `--x` skips both dashes when a long table exists; otherwise only
            // the first, leaving `-x` for the short decoder.
            if text.get(1) == Some(&b'-') && !longopts.is_null() {
                2
            } else {
                1
            }
        }
    };

    // A long option is only ever recognised at the start of an element, never
    // part-way through a short cluster.
    if resume.is_none() && !longopts.is_null() {
        // SAFETY: `optind` is in range.
        let text = unsafe { bytes(arg(argv, kinakaze_abi_optind.get())) };
        let double = text.get(1) == Some(&b'-');
        // With `long_only`, `-name` is tried as a long option too; a bare `-x`
        // that names a short option is left to the short decoder.
        let single_ok = long_only && (text.len() > 2 || find_short(stripped, text[1]).is_none());
        if double || single_ok {
            let context = LongScan {
                argc,
                argv,
                optstring: stripped,
                longopts,
                longindex,
                print_errors,
                colon,
                long_only,
            };
            // SAFETY: the table is null-terminated and `argv` holds `argc`
            // entries.
            if let Some(result) = unsafe { try_long(scan, &context, offset) } {
                return result;
            }
        }
    }

    // SAFETY: `optind` is in range and entries are null-terminated.
    let element = unsafe { arg(argv, kinakaze_abi_optind.get()) };
    // SAFETY: the entry is a null-terminated string.
    let text = unsafe { bytes(element) };
    let character = text[offset];

    // Advance past this character, and past the element when it is exhausted.
    let remaining = offset + 1;
    if remaining >= text.len() {
        scan.nextchar = None;
        kinakaze_abi_optind.set(kinakaze_abi_optind.get() + 1);
    } else {
        scan.nextchar = Some((kinakaze_abi_optind.get(), remaining));
    }

    let Some(position) = find_short(stripped, character) else {
        if print_errors {
            // SAFETY: `argv[0]` is null-terminated.
            let argv0 = unsafe { bytes(arg(argv, 0)) };
            report(argv0, &[b"invalid option -- '", &[character], b"'"]);
        }
        kinakaze_abi_optopt.set(c_int::from(character));
        return i32::from(b'?');
    };

    let takes = stripped.get(position + 1) == Some(&b':');
    let optional = takes && stripped.get(position + 2) == Some(&b':');

    if takes {
        if remaining < text.len() {
            // Attached: `-ofile`. The argument is the rest of this element.
            // SAFETY: `remaining` indexes within the element.
            kinakaze_abi_optarg.set(unsafe { element.add(remaining) });
            kinakaze_abi_optind.set(kinakaze_abi_optind.get() + 1);
            scan.nextchar = None;
        } else if optional {
            // An optional argument is never taken from the next element.
            kinakaze_abi_optarg.set(ptr::null_mut());
        } else if kinakaze_abi_optind.get() >= argc {
            if print_errors {
                // SAFETY: `argv[0]` is null-terminated.
                let argv0 = unsafe { bytes(arg(argv, 0)) };
                report(
                    argv0,
                    &[b"option requires an argument -- '", &[character], b"'"],
                );
            }
            kinakaze_abi_optopt.set(c_int::from(character));
            return i32::from(if colon { b':' } else { b'?' });
        } else {
            // Separate: `-o file`.
            // SAFETY: the index was just bounds-checked.
            kinakaze_abi_optarg.set(unsafe { arg(argv, kinakaze_abi_optind.get()) });
            kinakaze_abi_optind.set(kinakaze_abi_optind.get() + 1);
        }
    }

    c_int::from(character)
}

/// `getopt`.
///
/// # Safety
///
/// `argv` must hold `argc` null-terminated entries and `optstring` must be
/// null-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getopt(
    argc: c_int,
    argv: *mut *mut c_char,
    optstring: *const c_char,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { getopt_internal(argc, argv, optstring, ptr::null(), ptr::null_mut(), false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn getopt(
    argc: c_int,
    argv: *mut *mut c_char,
    optstring: *const c_char,
) -> c_int {
    unsafe { kinakaze_abi_getopt(argc, argv, optstring) }
}

/// `getopt_long`.
///
/// # Safety
///
/// In addition to `getopt`'s contract, `longopts` must be null or a table
/// terminated by an entry with a null `name`, and `longindex` must be null or
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getopt_long(
    argc: c_int,
    argv: *mut *mut c_char,
    optstring: *const c_char,
    longopts: *const LongOption,
    longindex: *mut c_int,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { getopt_internal(argc, argv, optstring, longopts, longindex, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn getopt_long(
    argc: c_int,
    argv: *mut *mut c_char,
    optstring: *const c_char,
    longopts: *const LongOption,
    longindex: *mut c_int,
) -> c_int {
    unsafe { kinakaze_abi_getopt_long(argc, argv, optstring, longopts, longindex) }
}

/// `getopt_long_only`, which also accepts long options written with one dash.
///
/// # Safety
///
/// The same contract as [`kinakaze_abi_getopt_long`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getopt_long_only(
    argc: c_int,
    argv: *mut *mut c_char,
    optstring: *const c_char,
    longopts: *const LongOption,
    longindex: *mut c_int,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { getopt_internal(argc, argv, optstring, longopts, longindex, true) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn getopt_long_only(
    argc: c_int,
    argv: *mut *mut c_char,
    optstring: *const c_char,
    longopts: *const LongOption,
    longindex: *mut c_int,
) -> c_int {
    unsafe { kinakaze_abi_getopt_long_only(argc, argv, optstring, longopts, longindex) }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::ffi::CStr;

    /// The exported variables and the scan state are process-global, so the
    /// tests take turns.
    ///
    /// `pub(crate)` because [`crate::copied`] has to take the same turn: its test
    /// installs a redirect on these very variables, and doing that while a scan is in
    /// progress on another thread would move `optind` out from under it.
    pub(crate) static SERIAL: Mutex<()> = Mutex::new(());

    pub(crate) fn serialise() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// An owned `argv` whose entries can be permuted in place.
    struct Argv {
        /// Backing storage for the entries; only the pointers are permuted.
        #[allow(dead_code)]
        owned: Vec<Vec<u8>>,
        pointers: Vec<*mut c_char>,
    }

    impl Argv {
        fn new(items: &[&str]) -> Self {
            let mut owned: Vec<Vec<u8>> = items
                .iter()
                .map(|item| {
                    let mut bytes = item.as_bytes().to_vec();
                    bytes.push(0);
                    bytes
                })
                .collect();
            let pointers = owned
                .iter_mut()
                .map(|item| item.as_mut_ptr().cast::<c_char>())
                .collect();
            Self { owned, pointers }
        }

        fn argc(&self) -> c_int {
            self.pointers.len() as c_int
        }

        fn as_ptr(&mut self) -> *mut *mut c_char {
            self.pointers.as_mut_ptr()
        }

        /// The current order of the entries, which permutation rearranges.
        fn order(&self) -> Vec<String> {
            self.pointers
                .iter()
                .map(|entry| {
                    // SAFETY: every entry addresses one of the owned buffers,
                    // each of which is null-terminated.
                    unsafe { CStr::from_ptr(*entry) }
                        .to_string_lossy()
                        .into_owned()
                })
                .collect()
        }
    }

    /// Requests a fresh scan and silences diagnostics.
    fn reset() {
        kinakaze_abi_optind.set(0);
        kinakaze_abi_opterr.set(0);
        kinakaze_abi_optopt.set(0);
        kinakaze_abi_optarg.set(ptr::null_mut());
    }

    fn optstring(text: &str) -> Vec<c_char> {
        let mut bytes: Vec<c_char> = text.bytes().map(|byte| byte as c_char).collect();
        bytes.push(0);
        bytes
    }

    /// The `optarg` of the option just returned.
    fn optarg() -> Option<String> {
        let value = kinakaze_abi_optarg.get();
        if value.is_null() {
            return None;
        }
        // SAFETY: `optarg` addresses a null-terminated `argv` entry.
        Some(
            unsafe { CStr::from_ptr(value) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    /// Runs a whole short-option scan, collecting what each call returned.
    fn scan_all(items: &[&str], spec: &str) -> (Vec<(c_int, Option<String>)>, Argv) {
        let mut argv = Argv::new(items);
        let spec = optstring(spec);
        let mut results = Vec::new();
        loop {
            // SAFETY: `argv` holds `argc` null-terminated entries and the
            // option string is null-terminated.
            let result = unsafe { kinakaze_abi_getopt(argv.argc(), argv.as_ptr(), spec.as_ptr()) };
            if result == -1 {
                break;
            }
            results.push((result, optarg()));
        }
        (results, argv)
    }

    /// The returned option characters, ignoring arguments.
    fn characters(results: &[(c_int, Option<String>)]) -> Vec<char> {
        results
            .iter()
            .map(|(value, _)| *value as u8 as char)
            .collect()
    }

    #[test]
    fn simple_flags_are_returned_in_order() {
        let _guard = serialise();
        reset();
        let (results, _argv) = scan_all(&["prog", "-a", "-b"], "ab");
        assert_eq!(characters(&results), ['a', 'b']);
    }

    #[test]
    fn a_cluster_yields_each_character() {
        let _guard = serialise();
        reset();
        let (results, _argv) = scan_all(&["prog", "-abc"], "abc");
        assert_eq!(characters(&results), ['a', 'b', 'c']);
        assert_eq!(kinakaze_abi_optind.get(), 2);
    }

    #[test]
    fn an_attached_argument_is_the_rest_of_the_element() {
        let _guard = serialise();
        reset();
        let (results, _argv) = scan_all(&["prog", "-ofile"], "o:");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'o'));
        assert_eq!(results[0].1.as_deref(), Some("file"));
    }

    #[test]
    fn a_separate_argument_is_the_next_element() {
        let _guard = serialise();
        reset();
        let (results, _argv) = scan_all(&["prog", "-o", "file"], "o:");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'o'));
        assert_eq!(results[0].1.as_deref(), Some("file"));
        assert_eq!(kinakaze_abi_optind.get(), 3);
    }

    #[test]
    fn a_cluster_may_end_in_an_option_taking_an_argument() {
        let _guard = serialise();
        reset();
        let (results, _argv) = scan_all(&["prog", "-ao", "file"], "ao:");
        assert_eq!(characters(&results), ['a', 'o']);
        assert_eq!(results[1].1.as_deref(), Some("file"));
    }

    #[test]
    fn a_missing_required_argument_reports_a_question_mark() {
        let _guard = serialise();
        reset();
        let (results, _argv) = scan_all(&["prog", "-o"], "o:");
        assert_eq!(characters(&results), ['?']);
        assert_eq!(kinakaze_abi_optopt.get(), i32::from(b'o'));
    }

    #[test]
    fn a_leading_colon_reports_a_missing_argument_as_colon() {
        let _guard = serialise();
        reset();
        let (results, _argv) = scan_all(&["prog", "-o"], ":o:");
        assert_eq!(characters(&results), [':']);
        assert_eq!(kinakaze_abi_optopt.get(), i32::from(b'o'));
    }

    #[test]
    fn an_optional_argument_may_be_absent() {
        let _guard = serialise();
        reset();
        let (results, _argv) = scan_all(&["prog", "-o", "operand"], "o::");
        assert_eq!(characters(&results), ['o']);
        assert_eq!(results[0].1, None, "an optional argument is never separate");
    }

    #[test]
    fn an_optional_argument_may_be_attached() {
        let _guard = serialise();
        reset();
        let (results, _argv) = scan_all(&["prog", "-ovalue"], "o::");
        assert_eq!(characters(&results), ['o']);
        assert_eq!(results[0].1.as_deref(), Some("value"));
    }

    #[test]
    fn an_unknown_option_reports_a_question_mark_and_sets_optopt() {
        let _guard = serialise();
        reset();
        let (results, _argv) = scan_all(&["prog", "-x"], "ab");
        assert_eq!(characters(&results), ['?']);
        assert_eq!(kinakaze_abi_optopt.get(), i32::from(b'x'));
    }

    #[test]
    fn a_double_dash_ends_the_options() {
        let _guard = serialise();
        reset();
        let (results, argv) = scan_all(&["prog", "-a", "--", "-b"], "ab");
        assert_eq!(characters(&results), ['a']);
        assert_eq!(
            kinakaze_abi_optind.get(),
            3,
            "optind must point past the delimiter"
        );
        assert_eq!(argv.order()[3], "-b");
    }

    #[test]
    fn a_lone_dash_is_an_operand() {
        let _guard = serialise();
        reset();
        let (results, argv) = scan_all(&["prog", "-a", "-"], "ab");
        assert_eq!(characters(&results), ['a']);
        // The lone `-` was permuted to the end and is where `optind` stops.
        assert_eq!(argv.order()[kinakaze_abi_optind.get() as usize], "-");
    }

    #[test]
    fn non_options_are_permuted_to_the_end() {
        let _guard = serialise();
        reset();
        let (results, argv) = scan_all(&["prog", "-a", "nonopt", "-b"], "ab");
        assert_eq!(characters(&results), ['a', 'b']);
        assert_eq!(
            argv.order(),
            vec!["prog", "-a", "-b", "nonopt"],
            "the operand must be rotated behind the options"
        );
        assert_eq!(
            kinakaze_abi_optind.get(),
            3,
            "optind must address the first operand"
        );
    }

    #[test]
    fn several_non_options_keep_their_relative_order() {
        let _guard = serialise();
        reset();
        let (results, argv) = scan_all(&["prog", "one", "-a", "two", "-b", "three"], "ab");
        assert_eq!(characters(&results), ['a', 'b']);
        assert_eq!(
            argv.order(),
            vec!["prog", "-a", "-b", "one", "two", "three"]
        );
        assert_eq!(kinakaze_abi_optind.get(), 3);
    }

    #[test]
    fn a_plus_prefix_stops_at_the_first_non_option() {
        let _guard = serialise();
        reset();
        let (results, argv) = scan_all(&["prog", "-a", "nonopt", "-b"], "+ab");
        assert_eq!(characters(&results), ['a']);
        assert_eq!(
            kinakaze_abi_optind.get(),
            2,
            "the scan must stop on the operand"
        );
        assert_eq!(
            argv.order(),
            vec!["prog", "-a", "nonopt", "-b"],
            "nothing is permuted when the scan stops"
        );
    }

    #[test]
    fn a_dash_prefix_returns_operands_as_character_one() {
        let _guard = serialise();
        reset();
        let (results, _argv) = scan_all(&["prog", "-a", "nonopt", "-b"], "-ab");
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0, i32::from(b'a'));
        assert_eq!(results[1].0, 1, "an operand is returned as character 1");
        assert_eq!(results[1].1.as_deref(), Some("nonopt"));
        assert_eq!(results[2].0, i32::from(b'b'));
    }

    #[test]
    fn a_reset_through_optind_starts_a_fresh_scan() {
        let _guard = serialise();
        reset();
        let (first, _argv) = scan_all(&["prog", "-a", "-b"], "ab");
        assert_eq!(characters(&first), ['a', 'b']);

        // A fresh scan of a different command line, as BusyBox does between
        // applets.
        kinakaze_abi_optind.set(0);
        let (second, _argv) = scan_all(&["prog", "-ofile", "operand"], "o:");
        assert_eq!(characters(&second), ['o']);
        assert_eq!(second[0].1.as_deref(), Some("file"));
        assert_eq!(kinakaze_abi_optind.get(), 2);
    }

    /// A second scan started with `optind = 1` rather than `0` must not inherit the
    /// first scan's recorded non-option run.
    ///
    /// `optind = 0` is the documented GNU reset and is covered above, but it is not
    /// the only one callers use: BusyBox's `getopt32` reinitialises by assigning
    /// `optind = 1`, which is what POSIX code has always done because it predates
    /// the GNU extension. That path skipped `initialise`, so `first_nonopt` and
    /// `last_nonopt` survived from the previous command line — and the
    /// end-of-scan branch then set `optind` back to that stale `first_nonopt`.
    ///
    /// Observably: `busybox ls` tried to open a file called `ls`, and
    /// `busybox cat /proc/version` opened both `cat` and the real file, because the
    /// applet collected operands from `argv[0]`.
    #[test]
    fn a_reset_through_optind_one_also_starts_a_fresh_scan() {
        let _guard = serialise();
        reset();

        // A first scan that records a non-option run: `operand` sits before `-b`,
        // so permuting leaves `first_nonopt` behind at a nonzero index.
        let (first, _argv) = scan_all(&["prog", "-a", "operand", "-b"], "ab");
        assert_eq!(characters(&first), ['a', 'b']);

        // Now reset the POSIX way and scan a command line with no options at all.
        // `optind` must end at 1, pointing at the first operand — not at whatever
        // the previous scan left in `first_nonopt`.
        kinakaze_abi_optind.set(1);
        let mut argv = Argv::new(&["ls"]);
        let spec = optstring("la");
        // SAFETY: `argv` holds `argc` null-terminated entries.
        let result = unsafe { kinakaze_abi_getopt(argv.argc(), argv.as_ptr(), spec.as_ptr()) };
        assert_eq!(result, -1, "no options to find");
        assert_eq!(
            kinakaze_abi_optind.get(),
            1,
            "optind must not point at argv[0]; the applet name would be read as an operand"
        );
    }

    #[test]
    fn a_partly_consumed_cluster_survives_a_reset() {
        let _guard = serialise();
        reset();
        let mut argv = Argv::new(&["prog", "-abc"]);
        let spec = optstring("abc");
        // SAFETY: `argv` holds `argc` null-terminated entries.
        let first = unsafe { kinakaze_abi_getopt(argv.argc(), argv.as_ptr(), spec.as_ptr()) };
        assert_eq!(first, i32::from(b'a'));

        // Abandoning the scan mid-cluster must not leak the offset into the
        // next one.
        kinakaze_abi_optind.set(0);
        let (results, _argv) = scan_all(&["prog", "-c"], "abc");
        assert_eq!(characters(&results), ['c']);
    }

    /// An owned long-option table, terminated by a null `name`.
    struct Table {
        /// Backing storage for the names the entries point at.
        #[allow(dead_code)]
        names: Vec<Vec<u8>>,
        entries: Vec<LongOption>,
    }

    impl Table {
        fn new(items: &[(&str, c_int, c_int)]) -> Self {
            let mut names: Vec<Vec<u8>> = items
                .iter()
                .map(|(name, _, _)| {
                    let mut bytes = name.as_bytes().to_vec();
                    bytes.push(0);
                    bytes
                })
                .collect();
            let mut entries: Vec<LongOption> = names
                .iter_mut()
                .zip(items)
                .map(|(name, (_, has_arg, val))| LongOption {
                    name: name.as_ptr().cast::<c_char>(),
                    has_arg: *has_arg,
                    flag: ptr::null_mut(),
                    val: *val,
                })
                .collect();
            entries.push(LongOption {
                name: ptr::null(),
                has_arg: 0,
                flag: ptr::null_mut(),
                val: 0,
            });
            Self { names, entries }
        }

        fn as_ptr(&self) -> *const LongOption {
            self.entries.as_ptr()
        }
    }

    /// Runs a whole `getopt_long` scan, recording the `longindex` of each match.
    fn scan_long(
        items: &[&str],
        spec: &str,
        table: &Table,
        only: bool,
    ) -> (Vec<(c_int, Option<String>, c_int)>, Argv) {
        let mut argv = Argv::new(items);
        let spec = optstring(spec);
        let mut results = Vec::new();
        loop {
            let mut index: c_int = -1;
            // SAFETY: `argv` holds `argc` null-terminated entries, the option
            // string is null-terminated, and the table ends with a null `name`.
            let result = unsafe {
                if only {
                    kinakaze_abi_getopt_long_only(
                        argv.argc(),
                        argv.as_ptr(),
                        spec.as_ptr(),
                        table.as_ptr(),
                        &raw mut index,
                    )
                } else {
                    kinakaze_abi_getopt_long(
                        argv.argc(),
                        argv.as_ptr(),
                        spec.as_ptr(),
                        table.as_ptr(),
                        &raw mut index,
                    )
                }
            };
            if result == -1 {
                break;
            }
            results.push((result, optarg(), index));
        }
        (results, argv)
    }

    fn standard_table() -> Table {
        Table::new(&[
            ("verbose", NO_ARGUMENT, i32::from(b'v')),
            ("version", NO_ARGUMENT, i32::from(b'V')),
            ("file", REQUIRED_ARGUMENT, i32::from(b'f')),
            ("colour", OPTIONAL_ARGUMENT, i32::from(b'c')),
        ])
    }

    #[test]
    fn a_long_option_returns_its_value() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        let (results, _argv) = scan_long(&["prog", "--verbose"], "vVf:c::", &table, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'v'));
        assert_eq!(results[0].2, 0, "longindex must name the matched entry");
    }

    #[test]
    fn a_long_option_takes_an_attached_argument() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        let (results, _argv) = scan_long(&["prog", "--file=x"], "vVf:c::", &table, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'f'));
        assert_eq!(results[0].1.as_deref(), Some("x"));
        assert_eq!(results[0].2, 2);
    }

    #[test]
    fn a_long_option_takes_a_separate_argument() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        let (results, _argv) = scan_long(&["prog", "--file", "x"], "vVf:c::", &table, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'f'));
        assert_eq!(results[0].1.as_deref(), Some("x"));
        assert_eq!(kinakaze_abi_optind.get(), 3);
    }

    #[test]
    fn an_unambiguous_abbreviation_matches() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        // `verb` can only be `verbose`; `version` does not share the prefix.
        let (results, _argv) = scan_long(&["prog", "--verb"], "vVf:c::", &table, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'v'));
    }

    #[test]
    fn an_abbreviation_may_carry_an_argument() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        let (results, _argv) = scan_long(&["prog", "--fi=x"], "vVf:c::", &table, false);
        assert_eq!(results[0].0, i32::from(b'f'));
        assert_eq!(results[0].1.as_deref(), Some("x"));
    }

    #[test]
    fn an_ambiguous_abbreviation_is_an_error() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        // `ver` prefixes both `verbose` and `version`.
        let (results, _argv) = scan_long(&["prog", "--ver"], "vVf:c::", &table, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'?'));
    }

    #[test]
    fn an_exact_match_beats_a_longer_candidate() {
        let _guard = serialise();
        reset();
        let table = Table::new(&[
            ("log", NO_ARGUMENT, i32::from(b'l')),
            ("logfile", REQUIRED_ARGUMENT, i32::from(b'L')),
        ]);
        let (results, _argv) = scan_long(&["prog", "--log"], "lL:", &table, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'l'), "the exact name must win");
    }

    #[test]
    fn an_unrecognised_long_option_is_an_error() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        let (results, _argv) = scan_long(&["prog", "--nope"], "vVf:c::", &table, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'?'));
        assert_eq!(kinakaze_abi_optind.get(), 2);
    }

    #[test]
    fn a_long_option_rejects_an_argument_it_does_not_take() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        let (results, _argv) = scan_long(&["prog", "--verbose=x"], "vVf:c::", &table, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'?'));
    }

    #[test]
    fn a_long_option_missing_a_required_argument_is_an_error() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        let (results, _argv) = scan_long(&["prog", "--file"], "vVf:c::", &table, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'?'));
    }

    #[test]
    fn a_long_optional_argument_is_not_taken_from_the_next_element() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        let (results, _argv) =
            scan_long(&["prog", "--colour", "operand"], "vVf:c::", &table, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'c'));
        assert_eq!(results[0].1, None);
    }

    #[test]
    fn a_flag_pointer_receives_the_value_and_zero_is_returned() {
        let _guard = serialise();
        reset();
        let mut slot: c_int = 0;
        let mut name = b"quiet\0".to_vec();
        let entries = [
            LongOption {
                name: name.as_mut_ptr().cast::<c_char>(),
                has_arg: NO_ARGUMENT,
                flag: &raw mut slot,
                val: 42,
            },
            LongOption {
                name: ptr::null(),
                has_arg: 0,
                flag: ptr::null_mut(),
                val: 0,
            },
        ];
        let mut argv = Argv::new(&["prog", "--quiet"]);
        let spec = optstring("");
        // SAFETY: the table ends with a null `name` and `argv` is well formed.
        let result = unsafe {
            kinakaze_abi_getopt_long(
                argv.argc(),
                argv.as_ptr(),
                spec.as_ptr(),
                entries.as_ptr(),
                ptr::null_mut(),
            )
        };
        assert_eq!(result, 0, "a table entry with a flag returns 0");
        assert_eq!(slot, 42, "the value is stored through the flag");
    }

    #[test]
    fn long_and_short_options_mix() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        let (results, argv) = scan_long(
            &["prog", "-v", "--file=x", "operand", "--verbose"],
            "vVf:c::",
            &table,
            false,
        );
        let returned: Vec<char> = results
            .iter()
            .map(|(value, _, _)| *value as u8 as char)
            .collect();
        assert_eq!(returned, ['v', 'f', 'v']);
        assert_eq!(argv.order().last().unwrap(), "operand");
        assert_eq!(kinakaze_abi_optind.get(), 4);
    }

    #[test]
    fn long_only_accepts_a_single_dash() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        let (results, _argv) = scan_long(&["prog", "-verbose"], "vVf:c::", &table, true);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'v'));
    }

    #[test]
    fn long_only_still_reads_a_known_short_option() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        // `-V` is a short option, so it is not looked up as a long name.
        let (results, _argv) = scan_long(&["prog", "-V"], "vVf:c::", &table, true);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'V'));
    }

    #[test]
    fn long_only_takes_a_single_dash_argument() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        let (results, _argv) = scan_long(&["prog", "-file", "x"], "vVf:c::", &table, true);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'f'));
        assert_eq!(results[0].1.as_deref(), Some("x"));
    }

    #[test]
    fn getopt_long_permutes_operands_too() {
        let _guard = serialise();
        reset();
        let table = standard_table();
        let (results, argv) =
            scan_long(&["prog", "operand", "--verbose"], "vVf:c::", &table, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, i32::from(b'v'));
        assert_eq!(argv.order(), vec!["prog", "--verbose", "operand"]);
        assert_eq!(kinakaze_abi_optind.get(), 2);
    }

    #[test]
    fn diagnostics_are_written_when_opterr_is_set() {
        let _guard = serialise();
        reset();
        // Exercise the reporting path itself; the message goes to descriptor 2.
        kinakaze_abi_opterr.set(1);
        let (results, _argv) = scan_all(&["prog", "-x"], "ab");
        kinakaze_abi_opterr.set(0);
        assert_eq!(characters(&results), ['?']);
    }
}
