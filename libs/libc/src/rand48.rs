//! System V 48-bit random generators with the Linux x86-64 GNU ABI.
//!
//! ABI and behavior references (independent Rust implementation):
//! https://codebrowser.dev/glibc/glibc/stdlib/stdlib.h.html
//! https://codebrowser.dev/glibc/glibc/stdlib/drand48-iter.c.html
//! https://codebrowser.dev/glibc/glibc/stdlib/seed48_r.c.html
//! https://codebrowser.dev/glibc/glibc/stdlib/srand48_r.c.html
//! https://codebrowser.dev/glibc/glibc/stdlib/lcong48_r.c.html

#![allow(clippy::missing_safety_doc)]

use core::cell::{RefCell, UnsafeCell};
use core::ptr;
use std::sync::{Mutex, MutexGuard, PoisonError};

const MASK: u64 = (1u64 << 48) - 1;
const MULTIPLIER: u64 = 0x5deece66d;
const ADDEND: u16 = 0xb;
const UNIT: f64 = 1.0 / ((1u64 << 48) as f64);

/// Linux `struct drand48_data`: 24 bytes, eight-byte aligned. Linux `long`
/// arguments/results below use i64, independent of the Windows C long width.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Drand48Data {
    pub __x: [u16; 3],
    pub __old_x: [u16; 3],
    pub __c: u16,
    pub __init: u16,
    pub __a: u64,
}

impl Drand48Data {
    const ZERO: Self = Self {
        __x: [0; 3],
        __old_x: [0; 3],
        __c: 0,
        __init: 0,
        __a: 0,
    };
}

// GNU's initial state is zero; lazy initialization sets the coefficients only.
// This storage never moves, including the previous seed returned by seed48.
struct GlobalState {
    lock: Mutex<()>,
    data: UnsafeCell<Drand48Data>,
}
// All operations on the shared generator hold lock. seed48's returned pointer
// follows the GNU API: callers must synchronize access with later seed changes.
unsafe impl Sync for GlobalState {}
static GLOBAL: GlobalState = GlobalState {
    lock: Mutex::new(()),
    data: UnsafeCell::new(Drand48Data::ZERO),
};

fn global() -> (MutexGuard<'static, ()>, *mut Drand48Data) {
    (
        GLOBAL.lock.lock().unwrap_or_else(PoisonError::into_inner),
        GLOBAL.data.get(),
    )
}

fn pack(words: [u16; 3]) -> u64 {
    u64::from(words[0]) | (u64::from(words[1]) << 16) | (u64::from(words[2]) << 32)
}

fn unpack(value: u64) -> [u16; 3] {
    [value as u16, (value >> 16) as u16, (value >> 32) as u16]
}

unsafe fn defaults(buffer: *mut Drand48Data) {
    unsafe {
        ptr::addr_of_mut!((*buffer).__a).write_unaligned(MULTIPLIER);
        ptr::addr_of_mut!((*buffer).__c).write_unaligned(ADDEND);
        ptr::addr_of_mut!((*buffer).__init).write_unaligned(1);
    }
}

// Raw field accesses permit the intentional xsubi == buffer->__x case and avoid
// reading unused/uninitialized fields of a buffer initialized by srand48_r.
unsafe fn advance(xsubi: *mut u16, buffer: *mut Drand48Data) -> u64 {
    unsafe {
        if ptr::addr_of!((*buffer).__init).read_unaligned() == 0 {
            defaults(buffer);
        }
        let multiplier = ptr::addr_of!((*buffer).__a).read_unaligned();
        let addend = ptr::addr_of!((*buffer).__c).read_unaligned();
        let old = pack(xsubi.cast::<[u16; 3]>().read_unaligned());
        let next = old.wrapping_mul(multiplier).wrapping_add(u64::from(addend)) & MASK;
        xsubi.cast::<[u16; 3]>().write_unaligned(unpack(next));
        next
    }
}

unsafe fn own_next(buffer: *mut Drand48Data) -> u64 {
    unsafe { advance(ptr::addr_of_mut!((*buffer).__x).cast(), buffer) }
}

fn signed(value: u64) -> i64 {
    i64::from((value >> 16) as u32 as i32)
}
fn positive(value: u64) -> i64 {
    (value >> 17) as i64
}

fn global_next() -> u64 {
    let (_guard, buffer) = global();
    unsafe { own_next(buffer) }
}

unsafe fn external_next(xsubi: *mut u16) -> Option<u64> {
    if xsubi.is_null() {
        kinakaze_tls::set_errno(crate::EINVAL);
        return None;
    }
    let (_guard, buffer) = global();
    Some(unsafe { advance(xsubi, buffer) })
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_drand48() -> f64 {
    global_next() as f64 * UNIT
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_lrand48() -> i64 {
    positive(global_next())
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_mrand48() -> i64 {
    signed(global_next())
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_erand48(xsubi: *mut u16) -> f64 {
    unsafe { external_next(xsubi) }.map_or(f64::NAN, |next| next as f64 * UNIT)
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_nrand48(xsubi: *mut u16) -> i64 {
    unsafe { external_next(xsubi) }.map_or(-1, positive)
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_jrand48(xsubi: *mut u16) -> i64 {
    unsafe { external_next(xsubi) }.map_or(-1, signed)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_drand48_r(
    buffer: *mut Drand48Data,
    result: *mut f64,
) -> i32 {
    if buffer.is_null() || result.is_null() {
        return -1;
    }
    unsafe { result.write_unaligned(own_next(buffer) as f64 * UNIT) };
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_lrand48_r(
    buffer: *mut Drand48Data,
    result: *mut i64,
) -> i32 {
    if buffer.is_null() || result.is_null() {
        return -1;
    }
    unsafe { result.write_unaligned(positive(own_next(buffer))) };
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mrand48_r(
    buffer: *mut Drand48Data,
    result: *mut i64,
) -> i32 {
    if buffer.is_null() || result.is_null() {
        return -1;
    }
    unsafe { result.write_unaligned(signed(own_next(buffer))) };
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_erand48_r(
    xsubi: *mut u16,
    buffer: *mut Drand48Data,
    result: *mut f64,
) -> i32 {
    if xsubi.is_null() || buffer.is_null() || result.is_null() {
        return -1;
    }
    unsafe { result.write_unaligned(advance(xsubi, buffer) as f64 * UNIT) };
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_nrand48_r(
    xsubi: *mut u16,
    buffer: *mut Drand48Data,
    result: *mut i64,
) -> i32 {
    if xsubi.is_null() || buffer.is_null() || result.is_null() {
        return -1;
    }
    unsafe { result.write_unaligned(positive(advance(xsubi, buffer))) };
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_jrand48_r(
    xsubi: *mut u16,
    buffer: *mut Drand48Data,
    result: *mut i64,
) -> i32 {
    if xsubi.is_null() || buffer.is_null() || result.is_null() {
        return -1;
    }
    unsafe { result.write_unaligned(signed(advance(xsubi, buffer))) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_srand48_r(seed: i64, buffer: *mut Drand48Data) -> i32 {
    if buffer.is_null() {
        return -1;
    }
    let words = [0x330e, seed as u16, ((seed as u64) >> 16) as u16];
    unsafe {
        ptr::addr_of_mut!((*buffer).__x).write_unaligned(words);
        defaults(buffer);
    }
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_seed48_r(
    seed: *const u16,
    buffer: *mut Drand48Data,
) -> i32 {
    if seed.is_null() || buffer.is_null() {
        return -1;
    }
    unsafe {
        ptr::copy(
            ptr::addr_of!((*buffer).__x).cast::<u8>(),
            ptr::addr_of_mut!((*buffer).__old_x).cast::<u8>(),
            6,
        );
        let words = seed.cast::<[u16; 3]>().read_unaligned();
        ptr::addr_of_mut!((*buffer).__x).write_unaligned(words);
        defaults(buffer);
    }
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_lcong48_r(
    params: *const u16,
    buffer: *mut Drand48Data,
) -> i32 {
    if params.is_null() || buffer.is_null() {
        return -1;
    }
    unsafe {
        let params = params.cast::<[u16; 7]>().read_unaligned();
        ptr::addr_of_mut!((*buffer).__x).write_unaligned([params[0], params[1], params[2]]);
        ptr::addr_of_mut!((*buffer).__a).write_unaligned(pack([params[3], params[4], params[5]]));
        ptr::addr_of_mut!((*buffer).__c).write_unaligned(params[6]);
        ptr::addr_of_mut!((*buffer).__init).write_unaligned(1);
    }
    0
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_srand48(seed: i64) {
    let (_guard, buffer) = global();
    unsafe { kinakaze_abi_srand48_r(seed, buffer) };
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_seed48(seed: *const u16) -> *mut u16 {
    if seed.is_null() {
        kinakaze_tls::set_errno(crate::EINVAL);
        return ptr::null_mut();
    }
    let (_guard, buffer) = global();
    unsafe {
        kinakaze_abi_seed48_r(seed, buffer);
        ptr::addr_of_mut!((*buffer).__old_x).cast()
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_lcong48(params: *const u16) {
    if params.is_null() {
        kinakaze_tls::set_errno(crate::EINVAL);
        return;
    }
    let (_guard, buffer) = global();
    unsafe { kinakaze_abi_lcong48_r(params, buffer) };
}

const FORK_MAGIC: u64 = 0x4352_5952_3438_4631; // CRYR48F1
const FORK_BYTES: usize = 32;
thread_local! {
    // The coordinator invokes prepare/snapshot/parent on the same thread. Keep
    // the parent generator frozen until success/rollback; never copy this lock
    // into the child DLL, which reconstructs only the scalar generator state.
    static FORK_GUARD: RefCell<Option<MutexGuard<'static, ()>>> = const { RefCell::new(None) };
}

fn encode_state(state: Drand48Data) -> [u8; FORK_BYTES] {
    let mut bytes = [0u8; FORK_BYTES];
    bytes[..8].copy_from_slice(&FORK_MAGIC.to_le_bytes());
    for (index, word) in state
        .__x
        .into_iter()
        .chain(state.__old_x)
        .chain([state.__c, state.__init])
        .enumerate()
    {
        bytes[8 + index * 2..10 + index * 2].copy_from_slice(&word.to_le_bytes());
    }
    bytes[24..].copy_from_slice(&state.__a.to_le_bytes());
    bytes
}

fn decode_state(bytes: &[u8]) -> Option<Drand48Data> {
    if bytes.len() != FORK_BYTES || u64::from_le_bytes(bytes[..8].try_into().ok()?) != FORK_MAGIC {
        return None;
    }
    let word = |index: usize| u16::from_le_bytes([bytes[8 + index * 2], bytes[9 + index * 2]]);
    Some(Drand48Data {
        __x: [word(0), word(1), word(2)],
        __old_x: [word(3), word(4), word(5)],
        __c: word(6),
        __init: word(7),
        __a: u64::from_le_bytes(bytes[24..].try_into().ok()?),
    })
}

unsafe extern "system" fn fork_prepare() -> i32 {
    FORK_GUARD.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return 35;
        }
        *slot = Some(global().0);
        0
    })
}

unsafe extern "system" fn fork_snapshot(output: *mut u8, capacity: usize) -> isize {
    if output.is_null() {
        return FORK_BYTES as isize;
    }
    if capacity < FORK_BYTES {
        return -(crate::EINVAL as isize);
    }
    FORK_GUARD.with(|slot| {
        if slot.borrow().is_none() {
            return -(crate::EINVAL as isize);
        }
        let bytes = encode_state(unsafe { GLOBAL.data.get().read() });
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len()) };
        FORK_BYTES as isize
    })
}

unsafe extern "system" fn fork_parent(_result: i32) {
    FORK_GUARD.with(|slot| {
        slot.borrow_mut().take();
    });
}

unsafe extern "system" fn fork_child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length != FORK_BYTES {
        return crate::EINVAL;
    }
    let Some(state) = decode_state(unsafe { core::slice::from_raw_parts(input, length) }) else {
        return crate::EINVAL;
    };
    let (_guard, buffer) = global();
    unsafe { buffer.write(state) };
    0
}

extern "C" fn register_fork_state() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        // User pthread prepare handlers (1000) run first; parent/child random
        // state is released/restored before those user callbacks run again.
        priority: 500,
        key: FORK_MAGIC,
        prepare: Some(fork_prepare),
        snapshot: Some(fork_snapshot),
        parent: Some(fork_parent),
        child: Some(fork_child),
    });
}

#[used]
#[unsafe(link_section = ".CRT$XCU")]
static FORK_INITIALIZER: extern "C" fn() = register_fork_state;

#[cfg(test)]
mod tests {
    use super::*;
    static GLOBAL_TEST: Mutex<()> = Mutex::new(());

    #[test]
    fn linux_data_layout_is_exact() {
        assert_eq!(size_of::<Drand48Data>(), 24);
        assert_eq!(align_of::<Drand48Data>(), 8);
        assert_eq!(core::mem::offset_of!(Drand48Data, __x), 0);
        assert_eq!(core::mem::offset_of!(Drand48Data, __old_x), 6);
        assert_eq!(core::mem::offset_of!(Drand48Data, __c), 12);
        assert_eq!(core::mem::offset_of!(Drand48Data, __init), 14);
        assert_eq!(core::mem::offset_of!(Drand48Data, __a), 16);
    }

    #[test]
    fn known_seed_zero_sequence_and_signed_results() {
        let mut floating = Drand48Data::ZERO;
        let mut positive_state = Drand48Data::ZERO;
        let mut signed_state = Drand48Data::ZERO;
        unsafe {
            assert_eq!(kinakaze_abi_srand48_r(0, &mut floating), 0);
            assert_eq!(kinakaze_abi_srand48_r(0, &mut positive_state), 0);
            assert_eq!(kinakaze_abi_srand48_r(0, &mut signed_state), 0);
        }
        let expected = [
            (0.17082803610628972, 366850414, 733700828),
            (0.7499019804849638, 1610402240, -1074162815),
            (0.09637165562356742, 206956554, 413913109),
            (0.8704652270270756, 1869309841, -556347614),
            (0.5773035067951078, 1239749840, -1815467615),
        ];
        for (double, nonnegative, signed) in expected {
            let (mut d, mut n, mut j) = (0.0, 0i64, 0i64);
            unsafe {
                assert_eq!(kinakaze_abi_drand48_r(&mut floating, &mut d), 0);
                assert_eq!(kinakaze_abi_lrand48_r(&mut positive_state, &mut n), 0);
                assert_eq!(kinakaze_abi_mrand48_r(&mut signed_state, &mut j), 0);
            }
            assert_eq!(d, double);
            assert_eq!(n, nonnegative);
            assert_eq!(j, signed);
            assert_eq!(floating.__x, positive_state.__x);
            assert_eq!(floating.__x, signed_state.__x);
        }
    }

    #[test]
    fn lazy_initialization_preserves_state_and_external_seed_ownership() {
        let mut zero = Drand48Data::ZERO;
        let mut d = 0.0;
        assert_eq!(unsafe { kinakaze_abi_drand48_r(&mut zero, &mut d) }, 0);
        assert_eq!(d, 11.0 * UNIT);
        assert_eq!(zero.__x, [11, 0, 0]);
        let mut buffer = Drand48Data {
            __x: [99, 88, 77],
            __old_x: [4, 5, 6],
            __a: 42,
            __c: 7,
            __init: 0,
        };
        let mut x = [1u16, 2, 3];
        assert_eq!(
            unsafe { kinakaze_abi_erand48_r(x.as_mut_ptr(), &mut buffer, &mut d) },
            0
        );
        assert_eq!(d, 0.44199632268870914);
        assert_eq!(x, [0xe678, 0xabc6, 0x7126]);
        assert_eq!(buffer.__x, [99, 88, 77]);
        assert_eq!(buffer.__old_x, [4, 5, 6]);
        assert_eq!(
            (buffer.__a, buffer.__c, buffer.__init),
            (MULTIPLIER, ADDEND, 1)
        );
        let mut n = 0;
        let mut j = 0;
        x = [1, 2, 3];
        assert_eq!(
            unsafe { kinakaze_abi_nrand48_r(x.as_mut_ptr(), &mut buffer, &mut n) },
            0
        );
        assert_eq!(n, 949179875);
        x = [u16::MAX; 3];
        assert_eq!(
            unsafe { kinakaze_abi_jrand48_r(x.as_mut_ptr(), &mut buffer, &mut j) },
            0
        );
        assert_eq!(j, -384749);
    }

    #[test]
    fn custom_coefficients_wrap_and_seeding_restores_defaults() {
        let mut buffer = Drand48Data::ZERO;
        let params = [u16::MAX; 7];
        assert_eq!(
            unsafe { kinakaze_abi_lcong48_r(params.as_ptr(), &mut buffer) },
            0
        );
        assert_eq!((buffer.__a, buffer.__c), (MASK, u16::MAX));
        let mut signed_result = 0;
        assert_eq!(
            unsafe { kinakaze_abi_mrand48_r(&mut buffer, &mut signed_result) },
            0
        );
        assert_eq!(signed_result, 1);
        assert_eq!(buffer.__x, [0, 1, 0]);
        let new_seed = [1u16, 2, 3];
        assert_eq!(
            unsafe { kinakaze_abi_seed48_r(new_seed.as_ptr(), &mut buffer) },
            0
        );
        assert_eq!(buffer.__old_x, [0, 1, 0]);
        assert_eq!(buffer.__x, new_seed);
        assert_eq!(
            (buffer.__a, buffer.__c, buffer.__init),
            (MULTIPLIER, ADDEND, 1)
        );
        assert_eq!(
            unsafe { kinakaze_abi_srand48_r(0x7abc_1234_5678, &mut buffer) },
            0
        );
        assert_eq!(buffer.__x, [0x330e, 0x5678, 0x1234]);
        assert_eq!(buffer.__old_x, [0, 1, 0]);
        assert_eq!(unsafe { kinakaze_abi_srand48_r(-1, &mut buffer) }, 0);
        assert_eq!(buffer.__x, [0x330e, 0xffff, 0xffff]);
    }

    #[test]
    fn srand_initializes_usable_fields_without_reading_prior_storage() {
        let mut storage = core::mem::MaybeUninit::<Drand48Data>::uninit();
        let buffer = storage.as_mut_ptr();
        assert_eq!(unsafe { kinakaze_abi_srand48_r(1, buffer) }, 0);
        let mut value = 0.0;
        assert_eq!(unsafe { kinakaze_abi_drand48_r(buffer, &mut value) }, 0);
        assert_eq!(value, 0.041630344771878214);
        let before = unsafe { ptr::addr_of!((*buffer).__x).read() };
        assert_eq!(
            unsafe { kinakaze_abi_drand48_r(buffer, ptr::null_mut()) },
            -1
        );
        assert_eq!(unsafe { ptr::addr_of!((*buffer).__x).read() }, before);
        assert_eq!(
            unsafe { kinakaze_abi_lrand48_r(ptr::null_mut(), ptr::null_mut()) },
            -1
        );
    }

    // This is the sole global-state test; every other test uses caller-owned
    // state so the ordinary parallel test runner cannot perturb this sequence.
    #[test]
    fn global_family_matches_reentrant_state_and_serializes_threads() {
        let _serial = GLOBAL_TEST.lock().unwrap();
        kinakaze_abi_srand48(0);
        assert_eq!(kinakaze_abi_drand48(), 0.17082803610628972);
        assert_eq!(kinakaze_abi_lrand48(), 1610402240);
        assert_eq!(kinakaze_abi_mrand48(), 413913109);
        kinakaze_abi_srand48(0x12345678);
        let seed = [1u16, 2, 3];
        let previous = unsafe { kinakaze_abi_seed48(seed.as_ptr()) };
        assert_eq!(
            unsafe { previous.cast::<[u16; 3]>().read() },
            [0x330e, 0x5678, 0x1234]
        );
        assert_eq!(kinakaze_abi_drand48(), 0.44199632268870914);
        assert_eq!(
            unsafe { previous.cast::<[u16; 3]>().read() },
            [0x330e, 0x5678, 0x1234]
        );
        let next_seed = [4u16, 5, 6];
        let same_storage = unsafe { kinakaze_abi_seed48(next_seed.as_ptr()) };
        assert_eq!(same_storage, previous);
        assert_eq!(
            unsafe { previous.cast::<[u16; 3]>().read() },
            [0xe678, 0xabc6, 0x7126]
        );

        // Caller-owned seeds use the global coefficients without moving its x.
        let custom = [0u16, 0, 0x8000, 1, 0, 0, 0];
        unsafe { kinakaze_abi_lcong48(custom.as_ptr()) };
        let mut external = [0xffffu16; 3];
        assert_eq!(unsafe { kinakaze_abi_jrand48(external.as_mut_ptr()) }, -1);
        assert_eq!(
            unsafe { kinakaze_abi_nrand48(external.as_mut_ptr()) },
            0x7fff_ffff
        );
        assert_eq!(
            unsafe { kinakaze_abi_erand48(external.as_mut_ptr()) },
            1.0 - UNIT
        );
        assert_eq!(kinakaze_abi_mrand48(), i64::from(i32::MIN));

        kinakaze_abi_srand48(1234);
        let threads = (0..8)
            .map(|_| {
                std::thread::spawn(|| (0..128).map(|_| kinakaze_abi_lrand48()).collect::<Vec<_>>())
            })
            .collect::<Vec<_>>();
        let mut observed = threads
            .into_iter()
            .flat_map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        let mut local = Drand48Data::ZERO;
        unsafe { kinakaze_abi_srand48_r(1234, &mut local) };
        let mut expected = (0..1024)
            .map(|_| {
                let mut output = 0;
                assert_eq!(
                    unsafe { kinakaze_abi_lrand48_r(&mut local, &mut output) },
                    0
                );
                output
            })
            .collect::<Vec<_>>();
        observed.sort_unstable();
        expected.sort_unstable();
        assert_eq!(observed, expected);
        assert_eq!(
            kinakaze_abi_lrand48(),
            positive(unsafe { own_next(&mut local) })
        );
    }

    #[test]
    fn fork_preserves_complete_global_state_and_releases_parent_on_rollback() {
        let _serial = GLOBAL_TEST.lock().unwrap();
        kinakaze_abi_srand48(42);
        let seed = [1u16, 2, 3];
        let previous_seed = unsafe { kinakaze_abi_seed48(seed.as_ptr()) };
        let params = [9u16, 8, 7, 0xffff, 0xabcd, 0x9876, 0x1234];
        unsafe { kinakaze_abi_lcong48(params.as_ptr()) };
        let before = {
            let (_guard, pointer) = global();
            unsafe { pointer.read() }
        };
        assert_eq!(before.__old_x, [0x330e, 42, 0]);
        assert_eq!(unsafe { fork_prepare() }, 0);
        assert_eq!(unsafe { fork_prepare() }, 35);
        let mut payload = [0u8; FORK_BYTES];
        assert_eq!(
            unsafe { fork_snapshot(ptr::null_mut(), 0) },
            FORK_BYTES as isize
        );
        assert_eq!(
            unsafe { fork_snapshot(payload.as_mut_ptr(), FORK_BYTES - 1) },
            -(crate::EINVAL as isize)
        );
        assert_eq!(
            unsafe { fork_snapshot(payload.as_mut_ptr(), payload.len()) },
            FORK_BYTES as isize
        );
        assert_eq!(decode_state(&payload), Some(before));
        let (entered, waiting) = std::sync::mpsc::channel();
        let (finished, result) = std::sync::mpsc::channel();
        let contender = std::thread::spawn(move || {
            entered.send(()).unwrap();
            finished.send(kinakaze_abi_lrand48()).unwrap();
        });
        waiting.recv().unwrap();
        assert_eq!(
            result.recv_timeout(std::time::Duration::from_millis(20)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
        unsafe { fork_parent(-1) };
        result
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        contender.join().unwrap();
        kinakaze_abi_srand48(99);
        assert_eq!(unsafe { fork_child(payload.as_ptr(), payload.len()) }, 0);
        assert_eq!(
            {
                let (_guard, pointer) = global();
                unsafe { pointer.read() }
            },
            before
        );
        assert_eq!(
            unsafe { previous_seed.cast::<[u16; 3]>().read() },
            before.__old_x
        );
        let mut child_expected = before;
        assert_eq!(
            kinakaze_abi_mrand48(),
            signed(unsafe { own_next(&mut child_expected) })
        );
        let mut corrupt = payload;
        corrupt[0] ^= 1;
        assert_eq!(
            unsafe { fork_child(corrupt.as_ptr(), corrupt.len()) },
            crate::EINVAL
        );
        assert_eq!(
            {
                let (_guard, pointer) = global();
                unsafe { pointer.read() }
            },
            child_expected
        );
    }
}
