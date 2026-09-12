//! GNU random_r state lives entirely in caller-owned Linux memory.
//! Algorithm and state format: glibc stdlib/random_r.c (TYPE_0..TYPE_4).
use core::{ffi::c_char, ptr};

const DEGREES: [usize; 5] = [0, 7, 15, 31, 63];
const SEPARATIONS: [usize; 5] = [0, 3, 1, 3, 1];

#[repr(C)]
pub struct RandomData {
    front: *mut i32,
    rear: *mut i32,
    state: *mut i32,
    kind: i32,
    degree: i32,
    separation: i32,
    end: *mut i32,
}

fn invalid() -> i32 {
    crate::set_errno(kinakaze_vfs::EINVAL);
    -1
}

unsafe fn save(data: &RandomData) {
    if !data.state.is_null() {
        let rear = if data.kind == 0 {
            0
        } else {
            (data.rear.addr() - data.state.addr()) / 4
        };
        unsafe {
            data.state
                .sub(1)
                .write_unaligned((rear * 5) as i32 + data.kind)
        };
    }
}

unsafe fn advance(data: &mut RandomData) -> i32 {
    if data.kind == 0 {
        let value = unsafe { data.state.read_unaligned() as u32 }
            .wrapping_mul(1_103_515_245)
            .wrapping_add(12_345)
            & 0x7fff_ffff;
        unsafe { data.state.write_unaligned(value as i32) };
        value as i32
    } else {
        let value = unsafe {
            (data.front.read_unaligned() as u32).wrapping_add(data.rear.read_unaligned() as u32)
        };
        unsafe {
            data.front.write_unaligned(value as i32);
            data.front = data.front.add(1);
            data.rear = data.rear.add(1);
        }
        if data.front == data.end {
            data.front = data.state;
        }
        if data.rear == data.end {
            data.rear = data.state;
        }
        (value >> 1) as i32
    }
}

unsafe fn seed(data: &mut RandomData, value: u32) {
    let value = if value == 0 { 1 } else { value };
    unsafe { data.state.write_unaligned(value as i32) };
    if data.kind == 0 {
        return;
    }
    let mut word = i64::from(value as i32);
    for index in 1..data.degree as usize {
        word = 16_807 * (word % 127_773) - 2_836 * (word / 127_773);
        if word < 0 {
            word += 2_147_483_647;
        }
        unsafe { data.state.add(index).write_unaligned(word as i32) };
    }
    data.front = unsafe { data.state.add(data.separation as usize) };
    data.rear = data.state;
    for _ in 0..data.degree * 10 {
        unsafe { advance(data) };
    }
}

/// Pointers must refer to the caller's writable random_data and state buffer.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_initstate_r(
    value: u32,
    state: *mut c_char,
    size: usize,
    data: *mut RandomData,
) -> i32 {
    if data.is_null() || state.is_null() || size < 8 {
        return invalid();
    }
    let kind = match size {
        0..32 => 0,
        32..64 => 1,
        64..128 => 2,
        128..256 => 3,
        _ => 4,
    };
    let data = unsafe { &mut *data };
    unsafe { save(data) };
    data.state = unsafe { state.cast::<i32>().add(1) };
    data.kind = kind as i32;
    data.degree = DEGREES[kind] as i32;
    data.separation = SEPARATIONS[kind] as i32;
    data.end = unsafe { data.state.add(DEGREES[kind]) };
    unsafe {
        seed(data, value);
        save(data);
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_random_r(
    data: *mut RandomData,
    result: *mut i32,
) -> i32 {
    if data.is_null() || result.is_null() {
        return invalid();
    }
    let data = unsafe { &mut *data };
    if data.state.is_null() || !(0..5).contains(&data.kind) {
        return invalid();
    }
    unsafe { result.write(advance(data)) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_srandom_r(value: u32, data: *mut RandomData) -> i32 {
    if data.is_null() {
        return invalid();
    }
    let data = unsafe { &mut *data };
    if data.state.is_null() || !(0..5).contains(&data.kind) {
        return invalid();
    }
    unsafe { seed(data, value) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setstate_r(
    state: *mut c_char,
    data: *mut RandomData,
) -> i32 {
    if state.is_null() || data.is_null() {
        return invalid();
    }
    let data = unsafe { &mut *data };
    // Save before reading the header, including setstate_r on the current state.
    unsafe { save(data) };
    let header = unsafe { state.cast::<i32>().read_unaligned() };
    if header < 0 {
        return invalid();
    }
    let kind = header as usize % 5;
    let rear = header as usize / 5;
    if (kind == 0 && rear != 0) || (kind != 0 && rear >= DEGREES[kind]) {
        return invalid();
    }
    let state = unsafe { state.cast::<i32>().add(1) };
    *data = RandomData {
        state,
        kind: kind as i32,
        degree: DEGREES[kind] as i32,
        separation: SEPARATIONS[kind] as i32,
        end: unsafe { state.add(DEGREES[kind]) },
        rear: if kind == 0 {
            ptr::null_mut()
        } else {
            unsafe { state.add(rear) }
        },
        front: if kind == 0 {
            ptr::null_mut()
        } else {
            unsafe { state.add((rear + SEPARATIONS[kind]) % DEGREES[kind]) }
        },
    };
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_glibc_sequences_all_state_sizes_and_unaligned_storage() {
        let expected = [
            1_103_527_590,
            964_237_963,
            1_894_937_090,
            1_804_289_383,
            510_644_794,
        ];
        for (index, size) in [8, 32, 64, 128, 256].into_iter().enumerate() {
            let mut storage = [0u8; 257];
            let mut data: RandomData = unsafe { core::mem::zeroed() };
            let mut result = -1;
            unsafe {
                assert_eq!(
                    kinakaze_abi_initstate_r(
                        1,
                        storage.as_mut_ptr().add(1).cast(),
                        size,
                        &mut data
                    ),
                    0
                );
                assert_eq!(kinakaze_abi_random_r(&mut data, &mut result), 0);
            }
            assert_eq!(result, expected[index]);
        }
        assert_eq!(core::mem::size_of::<RandomData>(), 48);
        assert_eq!(core::mem::offset_of!(RandomData, end), 40);
    }
}
