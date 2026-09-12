//! Restartable conversions for the selected C or C.UTF-8 locale.
//! Independent implementation of the GNU eight-byte mbstate ABI:
//! https://codebrowser.dev/glibc/glibc/wcsmbs/bits/types/__mbstate_t.h.html
//! https://codebrowser.dev/glibc/glibc/wcsmbs/mbrtoc16.c.html
//! https://codebrowser.dev/glibc/glibc/wcsmbs/c16rtomb.c.html
//! https://codebrowser.dev/glibc/glibc/iconv/gconv_simple.c.html

#![allow(clippy::missing_safety_doc)]

pub(crate) mod strings;
use core::cell::Cell;
use core::ffi::c_char;
use core::ptr;
use std::thread::LocalKey;

const INVALID: usize = usize::MAX;
const INCOMPLETE: usize = usize::MAX - 1;
const PENDING_OUTPUT: usize = usize::MAX - 2;
const SURROGATE: i32 = i32::MIN;
const EILSEQ: i32 = 84;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MbState {
    pub __count: i32,
    pub __value: u32,
}

impl MbState {
    const ZERO: Self = Self {
        __count: 0,
        __value: 0,
    };
}

thread_local! {
    static LENGTH: Cell<MbState> = const { Cell::new(MbState::ZERO) };
    static WIDE: Cell<MbState> = const { Cell::new(MbState::ZERO) };
    static DECODE32: Cell<MbState> = const { Cell::new(MbState::ZERO) };
    static DECODE16: Cell<MbState> = const { Cell::new(MbState::ZERO) };
    static ENCODE32: Cell<MbState> = const { Cell::new(MbState::ZERO) };
    static STRDECODE: Cell<MbState> = const { Cell::new(MbState::ZERO) };
    static STRENCODE: Cell<MbState> = const { Cell::new(MbState::ZERO) };
    static ENCODEWIDE: Cell<MbState> = const { Cell::new(MbState::ZERO) };
    static ENCODE16: Cell<MbState> = const { Cell::new(MbState::ZERO) };
}

unsafe fn with_state(
    state: *mut MbState,
    implicit: &'static LocalKey<Cell<MbState>>,
    action: impl FnOnce(&mut MbState) -> usize,
) -> usize {
    if state.is_null() {
        implicit.with(|slot| {
            let mut state = slot.get();
            let result = action(&mut state);
            slot.set(state);
            result
        })
    } else {
        // __value has no meaning in the initial state and need not be read.
        let count = unsafe { ptr::addr_of!((*state).__count).read_unaligned() };
        let value = if count == 0 {
            0
        } else {
            unsafe { ptr::addr_of!((*state).__value).read_unaligned() }
        };
        let mut current = MbState {
            __count: count,
            __value: value,
        };
        let result = action(&mut current);
        unsafe { state.write_unaligned(current) };
        result
    }
}

fn illegal() -> usize {
    kinakaze_tls::set_errno(EILSEQ);
    INVALID
}

fn prefix_valid(value: u32, have: u32, total: u32) -> bool {
    if !(2..=4).contains(&total) || have == 0 || have > total {
        return false;
    }
    let shift = 6 * (total - have);
    let Some(low) = value.checked_shl(shift) else {
        return false;
    };
    let high = low | ((1u32 << shift) - 1);
    let minimum = match total {
        2 => 0x80,
        3 => 0x800,
        _ => 0x10000,
    };
    let maximum = match total {
        2 => 0x7ff,
        3 => 0xffff,
        _ => 0x10ffff,
    };
    high >= minimum && low <= maximum && !(low >= 0xd800 && high <= 0xdfff)
}

fn unpack_partial(state: MbState) -> Option<(u32, u32, u32)> {
    let count = u32::try_from(state.__count).ok()?;
    let have = count & 0xff;
    let total = count >> 8;
    if !(2..=4).contains(&total) || have == 0 || have >= total {
        return None;
    }
    let shift = 6 * (total - have);
    if state.__value & ((1u32 << shift) - 1) != 0 {
        return None;
    }
    let value = state.__value >> shift;
    prefix_valid(value, have, total).then_some((value, have, total))
}

pub(crate) enum Decoded {
    Scalar(u32, usize),
    Incomplete,
    Invalid,
}

unsafe fn decode(input: *const c_char, count: usize, state: &mut MbState) -> Decoded {
    unsafe { decode_with_encoding(input, count, state, crate::locale::utf8()) }
}

pub(crate) unsafe fn decode_with_encoding(
    input: *const c_char,
    count: usize,
    state: &mut MbState,
    utf8: bool,
) -> Decoded {
    // Null input is the one-byte NUL sequence; an unfinished UTF-8 prefix can
    // therefore report EILSEQ instead of silently discarding pending bytes.
    let nul = 0u8;
    let (input, count) = if input.is_null() {
        (&raw const nul, 1)
    } else {
        (input.cast::<u8>(), count)
    };
    if count == 0 {
        return Decoded::Incomplete;
    }
    if !utf8 {
        let byte = unsafe { input.read() };
        if state.__count != 0 || byte >= 0x80 {
            return Decoded::Invalid;
        }
        *state = MbState::ZERO;
        return Decoded::Scalar(u32::from(byte), 1);
    }
    let mut consumed = 0usize;
    let (mut value, mut have, total) = if state.__count == 0 {
        let lead = unsafe { input.read() };
        consumed = 1;
        if lead < 0x80 {
            *state = MbState::ZERO;
            return Decoded::Scalar(u32::from(lead), 1);
        }
        match lead {
            0xc2..=0xdf => (u32::from(lead & 0x1f), 1, 2),
            0xe0..=0xef => (u32::from(lead & 0xf), 1, 3),
            0xf0..=0xf4 => (u32::from(lead & 7), 1, 4),
            _ => return Decoded::Invalid,
        }
    } else {
        let Some(partial) = unpack_partial(*state) else {
            return Decoded::Invalid;
        };
        partial
    };
    while have < total && consumed < count {
        let next = unsafe { input.add(consumed).read() };
        if next & 0xc0 != 0x80 {
            return Decoded::Invalid;
        }
        value = (value << 6) | u32::from(next & 0x3f);
        have += 1;
        consumed += 1;
        // Reject impossible prefixes immediately, including split overlong,
        // surrogate and > U+10FFFF sequences before the final byte arrives.
        if !prefix_valid(value, have, total) {
            return Decoded::Invalid;
        }
    }
    if have < total {
        // GNU UTF-8 mbstate stores total length in bits 8+, bytes consumed in
        // the low byte, and the partial scalar shifted for its missing bytes.
        *state = MbState {
            __count: ((total << 8) | have) as i32,
            __value: value << (6 * (total - have)),
        };
        Decoded::Incomplete
    } else {
        *state = MbState::ZERO;
        Decoded::Scalar(value, consumed)
    }
}

unsafe fn encode(output: *mut c_char, scalar: u32, state: &mut MbState) -> usize {
    unsafe { encode_with_encoding(output, scalar, state, crate::locale::utf8()) }
}

pub(crate) unsafe fn encode_with_encoding(
    output: *mut c_char,
    scalar: u32,
    state: &mut MbState,
    utf8: bool,
) -> usize {
    if output.is_null() || scalar == 0 {
        if !output.is_null() {
            unsafe { output.write(0) };
        }
        *state = MbState::ZERO;
        return 1;
    }
    if state.__count != 0 || (!utf8 && scalar >= 0x80) {
        return illegal();
    }
    let Some(value) = char::from_u32(scalar) else {
        return illegal();
    };
    let mut bytes = [0u8; 4];
    let encoded = value.encode_utf8(&mut bytes);
    unsafe { ptr::copy_nonoverlapping(encoded.as_ptr(), output.cast(), encoded.len()) };
    *state = MbState::ZERO;
    encoded.len()
}

unsafe fn decode_scalar(
    output: *mut u32,
    input: *const c_char,
    count: usize,
    state: *mut MbState,
    implicit: &'static LocalKey<Cell<MbState>>,
) -> usize {
    unsafe {
        with_state(state, implicit, |state| match decode(input, count, state) {
            Decoded::Scalar(value, consumed) => {
                if !output.is_null() && !input.is_null() {
                    output.write_unaligned(value);
                }
                if value == 0 { 0 } else { consumed }
            }
            Decoded::Incomplete => INCOMPLETE,
            Decoded::Invalid => illegal(),
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mbrtoc32(
    output: *mut u32,
    input: *const c_char,
    count: usize,
    state: *mut MbState,
) -> usize {
    unsafe { decode_scalar(output, input, count, state, &DECODE32) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mbrlen(
    input: *const c_char,
    count: usize,
    state: *mut MbState,
) -> usize {
    unsafe { decode_scalar(ptr::null_mut(), input, count, state, &LENGTH) }
}

// Linux wchar_t is a 32-bit scalar. Keep its implicit state independent of
// both mbrlen and mbrtoc32 while sharing explicit mbstate and UTF-8 validation.
pub unsafe fn mbrtowc(
    output: *mut i32,
    input: *const c_char,
    count: usize,
    state: *mut MbState,
) -> usize {
    unsafe { decode_scalar(output.cast(), input, count, state, &WIDE) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_c32rtomb(
    output: *mut c_char,
    scalar: u32,
    state: *mut MbState,
) -> usize {
    unsafe { with_state(state, &ENCODE32, |state| encode(output, scalar, state)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mbrtoc16(
    output: *mut u16,
    input: *const c_char,
    count: usize,
    state: *mut MbState,
) -> usize {
    unsafe {
        with_state(state, &DECODE16, |state| {
            if state.__count == SURROGATE {
                if !(0xdc00..=0xdfff).contains(&state.__value) {
                    return illegal();
                }
                if !output.is_null() {
                    output.write_unaligned(state.__value as u16);
                }
                *state = MbState::ZERO;
                return PENDING_OUTPUT;
            }
            match decode(input, count, state) {
                Decoded::Scalar(value, consumed) => {
                    let first = if value > 0xffff {
                        *state = MbState {
                            __count: SURROGATE,
                            __value: 0xdc00 | ((value - 0x10000) & 0x3ff),
                        };
                        (0xd800 | ((value - 0x10000) >> 10)) as u16
                    } else {
                        value as u16
                    };
                    if !output.is_null() && !input.is_null() {
                        output.write_unaligned(first);
                    }
                    if value == 0 { 0 } else { consumed }
                }
                Decoded::Incomplete => INCOMPLETE,
                Decoded::Invalid => illegal(),
            }
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_c16rtomb(
    output: *mut c_char,
    unit: u16,
    state: *mut MbState,
) -> usize {
    unsafe {
        with_state(state, &ENCODE16, |state| {
            if output.is_null() {
                *state = MbState::ZERO;
                return 1;
            }
            if !crate::locale::utf8() && unit >= 0x80 {
                return illegal();
            }
            let scalar = if state.__count == SURROGATE {
                let high = state.__value;
                *state = MbState::ZERO;
                if !(0xd800..=0xdbff).contains(&high) || !(0xdc00..=0xdfff).contains(&unit) {
                    return illegal();
                }
                0x10000 + ((high - 0xd800) << 10) + (u32::from(unit) - 0xdc00)
            } else {
                if state.__count != 0 {
                    return illegal();
                }
                if (0xd800..=0xdbff).contains(&unit) {
                    *state = MbState {
                        __count: SURROGATE,
                        __value: u32::from(unit),
                    };
                    return 0;
                }
                u32::from(unit)
            };
            encode(output, scalar, state)
        })
    }
}

/// `mbsinit` examines only the ABI count field, including pending UTF-16 output.
pub unsafe fn state_is_initial(state: *const MbState) -> bool {
    state.is_null() || unsafe { ptr::addr_of!((*state).__count).read_unaligned() == 0 }
}

// Explicit mbstate objects belong to guest memory. The implicit objects
// live in runtime-DLL TLS, so reconstruct the calling thread's objects on fork.
const FORK_MAGIC: u64 = 0x4352_5955_4348_4633; // CRYUCHF3
const IMPLICIT_STATES: usize = 9;
const FORK_BYTES: usize = 8 + IMPLICIT_STATES * 8;

fn implicit_states() -> [MbState; IMPLICIT_STATES] {
    [
        DECODE32.get(),
        DECODE16.get(),
        ENCODE32.get(),
        ENCODE16.get(),
        LENGTH.get(),
        WIDE.get(),
        STRDECODE.get(),
        STRENCODE.get(),
        ENCODEWIDE.get(),
    ]
}

fn set_implicit_states(states: [MbState; IMPLICIT_STATES]) {
    DECODE32.set(states[0]);
    DECODE16.set(states[1]);
    ENCODE32.set(states[2]);
    ENCODE16.set(states[3]);
    LENGTH.set(states[4]);
    WIDE.set(states[5]);
    STRDECODE.set(states[6]);
    STRENCODE.set(states[7]);
    ENCODEWIDE.set(states[8]);
}

fn valid_implicit_state(index: usize, state: MbState) -> bool {
    if state.__count == 0 {
        return true;
    }
    match index {
        0 | 4 | 5 | 6 => unpack_partial(state).is_some(),
        1 => {
            unpack_partial(state).is_some()
                || (state.__count == SURROGATE && (0xdc00..=0xdfff).contains(&state.__value))
        }
        3 => state.__count == SURROGATE && (0xd800..=0xdbff).contains(&state.__value),
        _ => false,
    }
}

unsafe extern "system" fn fork_snapshot(output: *mut u8, capacity: usize) -> isize {
    if output.is_null() {
        return FORK_BYTES as isize;
    }
    if capacity < FORK_BYTES {
        return -(crate::EINVAL as isize);
    }
    let mut bytes = [0u8; FORK_BYTES];
    bytes[..8].copy_from_slice(&FORK_MAGIC.to_le_bytes());
    for (index, state) in implicit_states().into_iter().enumerate() {
        let start = 8 + index * 8;
        bytes[start..start + 4].copy_from_slice(&state.__count.to_le_bytes());
        bytes[start + 4..start + 8].copy_from_slice(&state.__value.to_le_bytes());
    }
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len()) };
    FORK_BYTES as isize
}

unsafe extern "system" fn fork_child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length != FORK_BYTES {
        return crate::EINVAL;
    }
    let bytes = unsafe { core::slice::from_raw_parts(input, length) };
    if bytes[..8] != FORK_MAGIC.to_le_bytes() {
        return crate::EINVAL;
    }
    let mut states = [MbState::ZERO; IMPLICIT_STATES];
    for (index, state) in states.iter_mut().enumerate() {
        let start = 8 + index * 8;
        *state = MbState {
            __count: i32::from_le_bytes(bytes[start..start + 4].try_into().unwrap()),
            __value: u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap()),
        };
        if !valid_implicit_state(index, *state) {
            return crate::EINVAL;
        }
    }
    // Reject an entire malformed payload before changing any TLS object.
    set_implicit_states(states);
    0
}

extern "C" fn register_fork_state() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        // TLS owner restoration runs at 20; guest atfork callbacks run at 1000.
        priority: 25,
        key: FORK_MAGIC,
        prepare: None,
        snapshot: Some(fork_snapshot),
        parent: None,
        child: Some(fork_child),
    });
}

#[used]
#[unsafe(link_section = ".CRT$XCU")]
static FORK_INITIALIZER: extern "C" fn() = register_fork_state;

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn decode32(output: &mut u32, bytes: &[u8], state: &mut MbState) -> usize {
        unsafe { kinakaze_abi_mbrtoc32(output, bytes.as_ptr().cast(), bytes.len(), state) }
    }

    #[test]
    fn linux_mbstate_layout_and_mbsinit_observe_pending_state() {
        let _locale = crate::locale::TestLocale::new(true);
        assert_eq!(size_of::<MbState>(), 8);
        assert_eq!(align_of::<MbState>(), 4);
        assert_eq!(core::mem::offset_of!(MbState, __count), 0);
        assert_eq!(core::mem::offset_of!(MbState, __value), 4);
        let mut state = MbState {
            __count: 0,
            __value: 0xdeadbeef,
        };
        unsafe {
            assert!(state_is_initial(ptr::null()));
            assert!(state_is_initial(&state));
            assert_eq!(
                crate::strextra::windows::kinakaze_abi_mbsinit((&raw const state).cast()),
                1
            );
            let mut output = 0;
            assert_eq!(decode32(&mut output, &[0xe2], &mut state), INCOMPLETE);
            assert_eq!(
                state,
                MbState {
                    __count: 0x301,
                    __value: 0x2000
                }
            );
            assert_eq!(
                crate::strextra::windows::kinakaze_abi_mbsinit((&raw const state).cast()),
                0
            );
            assert_eq!(decode32(&mut output, &[0x82], &mut state), INCOMPLETE);
            assert_eq!(
                state,
                MbState {
                    __count: 0x302,
                    __value: 0x2080
                }
            );
            assert_eq!(decode32(&mut output, &[0xac], &mut state), 1);
            assert_eq!(output, 0x20ac);
            assert!(state_is_initial(&state));
        }
    }

    #[test]
    fn utf8_boundary_scalars_roundtrip_across_every_split() {
        let _locale = crate::locale::TestLocale::new(true);
        let scalars = [
            0, 1, 0x7f, 0x80, 0x7ff, 0x800, 0xd7ff, 0xe000, 0xffff, 0x10000, 0x1f642, 0x10ffff,
        ];
        for scalar in scalars {
            let value = char::from_u32(scalar).unwrap();
            let mut expected = [0u8; 4];
            let expected = value.encode_utf8(&mut expected).as_bytes();
            let mut encoded = [0xccu8; 8];
            let mut state = MbState::ZERO;
            unsafe {
                assert_eq!(
                    kinakaze_abi_c32rtomb(encoded.as_mut_ptr().cast(), scalar, &mut state),
                    expected.len()
                );
                assert_eq!(&encoded[..expected.len()], expected);
                assert_eq!(encoded[expected.len()], 0xcc);
                for split in 0..expected.len() {
                    let mut state = MbState::ZERO;
                    let mut output = u32::MAX;
                    assert_eq!(
                        decode32(&mut output, &expected[..split], &mut state),
                        INCOMPLETE
                    );
                    assert_eq!(output, u32::MAX);
                    assert_eq!(
                        decode32(&mut output, &expected[split..], &mut state),
                        if scalar == 0 {
                            0
                        } else {
                            expected.len() - split
                        }
                    );
                    assert_eq!(output, scalar);
                    assert!(state_is_initial(&state));
                }
                let mut output = u32::MAX;
                assert_eq!(
                    decode32(&mut output, expected, &mut state),
                    if scalar == 0 { 0 } else { expected.len() }
                );
                assert_eq!(output, scalar);
            }
        }
    }

    #[test]
    fn invalid_utf8_sets_eilseq_without_output() {
        let _locale = crate::locale::TestLocale::new(true);
        let cases: &[&[u8]] = &[
            &[0x80],
            &[0xc0, 0x80],
            &[0xc1, 0xbf],
            &[0xc2, 0x7f],
            &[0xe0, 0x80],
            &[0xe0, 0x9f, 0xbf],
            &[0xed, 0xa0],
            &[0xed, 0xbf, 0xbf],
            &[0xf0, 0x80],
            &[0xf0, 0x8f, 0xbf, 0xbf],
            &[0xf4, 0x90],
            &[0xf4, 0xbf, 0xbf, 0xbf],
            &[0xf5],
            &[0xff],
            &[0xe2, 0x82, b'X'],
        ];
        for bytes in cases {
            let mut state = MbState::ZERO;
            let mut output = 0xdeadbeef;
            kinakaze_tls::set_errno(0);
            assert_eq!(
                unsafe { decode32(&mut output, bytes, &mut state) },
                INVALID,
                "{bytes:x?}"
            );
            assert_eq!(kinakaze_tls::errno(), EILSEQ);
            assert_eq!(output, 0xdeadbeef);
            if (0xc2..=0xf4).contains(&bytes[0]) && bytes.len() > 1 {
                state = MbState::ZERO;
                assert_eq!(
                    unsafe { decode32(&mut output, &bytes[..1], &mut state) },
                    INCOMPLETE
                );
                kinakaze_tls::set_errno(0);
                assert_eq!(
                    unsafe { decode32(&mut output, &bytes[1..], &mut state) },
                    INVALID
                );
                assert_eq!(kinakaze_tls::errno(), EILSEQ);
            }
        }
    }

    #[test]
    fn null_and_zero_length_arguments_have_distinct_semantics() {
        let _locale = crate::locale::TestLocale::new(true);
        let mut state = MbState::ZERO;
        let mut output = 123;
        unsafe {
            assert_eq!(
                kinakaze_abi_mbrtoc32(&mut output, ptr::null(), 999, &mut state),
                0
            );
            assert_eq!(output, 123);
            assert_eq!(decode32(&mut output, &[0xe2], &mut state), INCOMPLETE);
            let saved = state;
            assert_eq!(decode32(&mut output, &[], &mut state), INCOMPLETE);
            assert_eq!(state, saved);
            assert_eq!(
                kinakaze_abi_mbrtoc32(&mut output, ptr::null(), 0, &mut state),
                INVALID
            );
            state = MbState::ZERO;
            assert_eq!(
                kinakaze_abi_mbrtoc32(ptr::null_mut(), c"A".as_ptr(), 1, &mut state),
                1
            );
            assert_eq!(
                kinakaze_abi_mbrtoc32(ptr::null_mut(), c"".as_ptr(), 1, &mut state),
                0
            );
            assert!(state_is_initial(&state));
            state = saved;
            assert_eq!(
                kinakaze_abi_c32rtomb(ptr::null_mut(), 0xffffffff, &mut state),
                1
            );
            assert!(state_is_initial(&state));
        }
    }

    #[test]
    fn invalid_char32_never_writes_output() {
        let _locale = crate::locale::TestLocale::new(true);
        for scalar in [0xd800, 0xdfff, 0x110000, u32::MAX] {
            let mut output = [0x55u8; 8];
            let mut state = MbState::ZERO;
            kinakaze_tls::set_errno(0);
            assert_eq!(
                unsafe { kinakaze_abi_c32rtomb(output.as_mut_ptr().cast(), scalar, &mut state) },
                INVALID
            );
            assert_eq!(output, [0x55; 8]);
            assert_eq!(kinakaze_tls::errno(), EILSEQ);
        }
    }

    #[test]
    fn char16_pending_surrogate_does_not_consume_input() {
        let _locale = crate::locale::TestLocale::new(true);
        let bytes = [0xf0u8, 0x9f, 0x99, 0x82];
        let mut state = MbState::ZERO;
        let mut output = 0u16;
        unsafe {
            assert_eq!(
                kinakaze_abi_mbrtoc16(&mut output, bytes.as_ptr().cast(), 4, &mut state),
                4
            );
            assert_eq!(output, 0xd83d);
            assert!(!state_is_initial(&state));
            assert_eq!(
                kinakaze_abi_mbrtoc16(&mut output, c"A".as_ptr(), 1, &mut state),
                PENDING_OUTPUT
            );
            assert_eq!(output, 0xde42);
            assert!(state_is_initial(&state));
            assert_eq!(
                kinakaze_abi_mbrtoc16(&mut output, c"A".as_ptr(), 1, &mut state),
                1
            );
            assert_eq!(output, 65);
            assert_eq!(
                kinakaze_abi_mbrtoc16(ptr::null_mut(), bytes.as_ptr().cast(), 4, &mut state),
                4
            );
            assert_eq!(
                kinakaze_abi_mbrtoc16(&mut output, ptr::null(), 0, &mut state),
                PENDING_OUTPUT
            );
            assert_eq!(output, 0xde42);
            assert_eq!(
                kinakaze_abi_mbrtoc16(&mut output, ptr::null(), 0, &mut state),
                0
            );
            assert_eq!(output, 0xde42);
            assert_eq!(
                kinakaze_abi_mbrtoc16(&mut output, c"".as_ptr(), 1, &mut state),
                0
            );
            assert_eq!(output, 0);
        }
    }

    #[test]
    fn char16_encode_surrogate_pair_reset_and_errors() {
        let _locale = crate::locale::TestLocale::new(true);
        let mut state = MbState::ZERO;
        let mut bytes = [0x55u8; 8];
        unsafe {
            assert_eq!(
                kinakaze_abi_c16rtomb(bytes.as_mut_ptr().cast(), 0xd83d, &mut state),
                0
            );
            assert_eq!(bytes, [0x55; 8]);
            assert!(!state_is_initial(&state));
            assert_eq!(
                kinakaze_abi_c16rtomb(bytes.as_mut_ptr().cast(), 0xde42, &mut state),
                4
            );
            assert_eq!(&bytes[..4], &[0xf0, 0x9f, 0x99, 0x82]);
            assert!(state_is_initial(&state));
            assert_eq!(
                kinakaze_abi_c16rtomb(bytes.as_mut_ptr().cast(), 0xde42, &mut state),
                INVALID
            );
            assert_eq!(kinakaze_tls::errno(), EILSEQ);
            assert_eq!(
                kinakaze_abi_c16rtomb(bytes.as_mut_ptr().cast(), 0xd800, &mut state),
                0
            );
            assert_eq!(
                kinakaze_abi_c16rtomb(bytes.as_mut_ptr().cast(), 65, &mut state),
                INVALID
            );
            assert!(state_is_initial(&state));
            assert_eq!(
                kinakaze_abi_c16rtomb(bytes.as_mut_ptr().cast(), 65, &mut state),
                1
            );
            assert_eq!(bytes[0], 65);
            assert_eq!(
                kinakaze_abi_c16rtomb(bytes.as_mut_ptr().cast(), 0xd800, &mut state),
                0
            );
            assert_eq!(
                kinakaze_abi_c16rtomb(ptr::null_mut(), 0xdc00, &mut state),
                1
            );
            assert!(state_is_initial(&state));
            assert_eq!(
                kinakaze_abi_c16rtomb(bytes.as_mut_ptr().cast(), 0, &mut state),
                1
            );
            assert_eq!(bytes[0], 0);
        }
    }

    #[test]
    fn implicit_states_are_separate_per_routine_and_thread() {
        let _locale = crate::locale::TestLocale::new(true);
        set_implicit_states([MbState::ZERO; IMPLICIT_STATES]);
        unsafe {
            let mut output = 0u32;
            assert_eq!(
                kinakaze_abi_mbrtoc32(&mut output, [0xe2u8].as_ptr().cast(), 1, ptr::null_mut()),
                INCOMPLETE
            );
            let mut output16 = 0;
            assert_eq!(
                kinakaze_abi_mbrtoc16(&mut output16, c"B".as_ptr(), 1, ptr::null_mut()),
                1
            );
            assert_eq!(output16, 66);
            std::thread::spawn(|| {
                let mut output = 0;
                assert_eq!(
                    kinakaze_abi_mbrtoc32(&mut output, c"C".as_ptr(), 1, ptr::null_mut()),
                    1
                );
                assert_eq!(output, 67);
            })
            .join()
            .unwrap();
            assert_eq!(
                kinakaze_abi_mbrtoc32(
                    &mut output,
                    [0x82u8, 0xac].as_ptr().cast(),
                    2,
                    ptr::null_mut()
                ),
                2
            );
            assert_eq!(output, 0x20ac);
            let mut bytes = [0u8; 4];
            assert_eq!(
                kinakaze_abi_c16rtomb(bytes.as_mut_ptr().cast(), 0xd83d, ptr::null_mut()),
                0
            );
            assert_eq!(
                kinakaze_abi_c32rtomb(bytes.as_mut_ptr().cast(), 65, ptr::null_mut()),
                1
            );
            assert_eq!(
                kinakaze_abi_c16rtomb(bytes.as_mut_ptr().cast(), 0xde42, ptr::null_mut()),
                4
            );
            assert_eq!(bytes, [0xf0, 0x9f, 0x99, 0x82]);
        }
        set_implicit_states([MbState::ZERO; IMPLICIT_STATES]);
    }

    #[test]
    fn fork_roundtrip_restores_all_implicit_states_atomically() {
        let _locale = crate::locale::TestLocale::new(true);
        set_implicit_states([MbState::ZERO; IMPLICIT_STATES]);
        unsafe {
            let mut output = 0;
            assert_eq!(
                kinakaze_abi_mbrtoc32(&mut output, [0xe2u8].as_ptr().cast(), 1, ptr::null_mut()),
                INCOMPLETE
            );
            assert_eq!(
                kinakaze_abi_mbrtoc16(
                    ptr::null_mut(),
                    [0xf0u8, 0x9f, 0x99, 0x82].as_ptr().cast(),
                    4,
                    ptr::null_mut()
                ),
                4
            );
            let mut encoded = [0u8; 4];
            assert_eq!(
                kinakaze_abi_c16rtomb(encoded.as_mut_ptr().cast(), 0xd83d, ptr::null_mut()),
                0
            );
            assert_eq!(
                kinakaze_abi_mbrlen([0xe2u8].as_ptr().cast(), 1, ptr::null_mut()),
                INCOMPLETE
            );
            assert_eq!(
                mbrtowc(
                    ptr::null_mut(),
                    [0xc2u8].as_ptr().cast(),
                    1,
                    ptr::null_mut()
                ),
                INCOMPLETE
            );
            let before = implicit_states();
            let mut snapshot = [0u8; FORK_BYTES];
            assert_eq!(fork_snapshot(ptr::null_mut(), 0), FORK_BYTES as isize);
            assert_eq!(
                fork_snapshot(snapshot.as_mut_ptr(), FORK_BYTES - 1),
                -(crate::EINVAL as isize)
            );
            assert_eq!(
                fork_snapshot(snapshot.as_mut_ptr(), FORK_BYTES),
                FORK_BYTES as isize
            );
            set_implicit_states([MbState::ZERO; IMPLICIT_STATES]);
            let mut invalid = snapshot;
            invalid[32..36].copy_from_slice(&1i32.to_le_bytes());
            assert_eq!(fork_child(invalid.as_ptr(), FORK_BYTES), crate::EINVAL);
            assert_eq!(implicit_states(), [MbState::ZERO; IMPLICIT_STATES]);
            assert_eq!(fork_child(snapshot.as_ptr(), FORK_BYTES - 1), crate::EINVAL);
            assert_eq!(fork_child(snapshot.as_ptr(), FORK_BYTES), 0);
            assert_eq!(implicit_states(), before);
            assert_eq!(
                kinakaze_abi_mbrtoc32(
                    &mut output,
                    [0x82u8, 0xac].as_ptr().cast(),
                    2,
                    ptr::null_mut()
                ),
                2
            );
            assert_eq!(output, 0x20ac);
            let mut output16 = 0;
            assert_eq!(
                kinakaze_abi_mbrtoc16(&mut output16, ptr::null(), 0, ptr::null_mut()),
                PENDING_OUTPUT
            );
            assert_eq!(output16, 0xde42);
            assert_eq!(
                kinakaze_abi_c16rtomb(encoded.as_mut_ptr().cast(), 0xde42, ptr::null_mut()),
                4
            );
            assert_eq!(encoded, [0xf0, 0x9f, 0x99, 0x82]);
            assert_eq!(
                kinakaze_abi_mbrlen([0x82u8, 0xac].as_ptr().cast(), 2, ptr::null_mut()),
                2
            );
            let mut wide = 0;
            assert_eq!(
                mbrtowc(&mut wide, [0xa3u8].as_ptr().cast(), 1, ptr::null_mut()),
                1
            );
            assert_eq!(wide, 0xa3);
            assert_eq!(implicit_states(), [MbState::ZERO; IMPLICIT_STATES]);
        }
    }
}
