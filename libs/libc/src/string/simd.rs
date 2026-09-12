//! Bounded scans: x86-64 SSE2 baseline, AVX2 for longer inputs when available.
//! Every vector stays inside the supplied range, including at guard pages.
use core::arch::x86_64::*;

pub(super) unsafe fn find_byte(bytes: *const u8, needle: u8, count: usize) -> Option<usize> {
    if count >= 64 && std::is_x86_feature_detected!("avx2") {
        return unsafe { find_byte_avx2(bytes, needle, count) };
    }
    unsafe { find_byte_sse2(bytes, needle, count) }
}

unsafe fn find_byte_sse2(bytes: *const u8, needle: u8, count: usize) -> Option<usize> {
    unsafe {
        let target = _mm_set1_epi8(needle as i8);
        let mut offset = 0;
        while count - offset >= 16 {
            let block = _mm_loadu_si128(bytes.add(offset).cast());
            let mask = _mm_movemask_epi8(_mm_cmpeq_epi8(block, target)) as u32;
            if mask != 0 {
                return Some(offset + mask.trailing_zeros() as usize);
            }
            offset += 16;
        }
        while offset < count {
            if *bytes.add(offset) == needle {
                return Some(offset);
            }
            offset += 1;
        }
        None
    }
}

#[target_feature(enable = "avx2")]
unsafe fn find_byte_avx2(bytes: *const u8, needle: u8, count: usize) -> Option<usize> {
    unsafe {
        let target = _mm256_set1_epi8(needle as i8);
        let mut offset = 0;
        while count - offset >= 32 {
            let block = _mm256_loadu_si256(bytes.add(offset).cast());
            let mask = _mm256_movemask_epi8(_mm256_cmpeq_epi8(block, target)) as u32;
            if mask != 0 {
                return Some(offset + mask.trailing_zeros() as usize);
            }
            offset += 32;
        }
        find_byte_sse2(bytes.add(offset), needle, count - offset).map(|tail| offset + tail)
    }
}

pub(super) unsafe fn first_mismatch(
    left: *const u8,
    right: *const u8,
    count: usize,
) -> Option<usize> {
    if count >= 64 && std::is_x86_feature_detected!("avx2") {
        return unsafe { first_mismatch_avx2(left, right, count) };
    }
    unsafe { first_mismatch_sse2(left, right, count) }
}

unsafe fn first_mismatch_sse2(left: *const u8, right: *const u8, count: usize) -> Option<usize> {
    unsafe {
        let mut offset = 0;
        while count - offset >= 16 {
            let a = _mm_loadu_si128(left.add(offset).cast());
            let b = _mm_loadu_si128(right.add(offset).cast());
            let different = !(_mm_movemask_epi8(_mm_cmpeq_epi8(a, b)) as u32) & 0xffff;
            if different != 0 {
                return Some(offset + different.trailing_zeros() as usize);
            }
            offset += 16;
        }
        while offset < count {
            if *left.add(offset) != *right.add(offset) {
                return Some(offset);
            }
            offset += 1;
        }
        None
    }
}

#[target_feature(enable = "avx2")]
unsafe fn first_mismatch_avx2(left: *const u8, right: *const u8, count: usize) -> Option<usize> {
    unsafe {
        let mut offset = 0;
        while count - offset >= 32 {
            let a = _mm256_loadu_si256(left.add(offset).cast());
            let b = _mm256_loadu_si256(right.add(offset).cast());
            let different = !(_mm256_movemask_epi8(_mm256_cmpeq_epi8(a, b)) as u32);
            if different != 0 {
                return Some(offset + different.trailing_zeros() as usize);
            }
            offset += 32;
        }
        first_mismatch_sse2(left.add(offset), right.add(offset), count - offset)
            .map(|tail| offset + tail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_match_scalar_for_unaligned_inputs_and_every_mismatch_position() {
        // Exercise both CPU paths even on a machine where dispatch uses AVX2.
        let avx2 = std::is_x86_feature_detected!("avx2");
        for alignment in 0..32 {
            for count in [0, 1, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 257] {
                let mut left = vec![0x80; count + alignment + 1];
                let right = vec![0x80; count + 33];
                let a = unsafe { left.as_mut_ptr().add(alignment) };
                let b = unsafe { right.as_ptr().add(31 - alignment) };
                unsafe {
                    assert_eq!(first_mismatch_sse2(a, b, count), None);
                    assert_eq!(find_byte_sse2(a, 0xff, count), None);
                    if avx2 {
                        assert_eq!(first_mismatch_avx2(a, b, count), None);
                        assert_eq!(find_byte_avx2(a, 0xff, count), None);
                    }
                    for index in 0..count {
                        *a.add(index) = 0xff;
                        assert_eq!(find_byte_sse2(a, 0xff, count), Some(index));
                        assert_eq!(first_mismatch_sse2(a, b, count), Some(index));
                        if avx2 {
                            assert_eq!(find_byte_avx2(a, 0xff, count), Some(index));
                            assert_eq!(first_mismatch_avx2(a, b, count), Some(index));
                        }
                        *a.add(index) = 0x80;
                    }
                }
            }
        }
    }
}
