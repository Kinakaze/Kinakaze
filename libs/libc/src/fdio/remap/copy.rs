//! Local committed VMA copies. The caller holds the mapping transaction and
//! makes source/destination readable/writable before using these SIMD loops.
use core::arch::x86_64::*;

#[inline]
pub(super) unsafe fn copy(source: *const u8, destination: *mut u8, length: usize) {
    // Retain the compiler/CRT's tuned SIMD/REP MOVSB outside the measured
    // medium range. In particular, manual AVX2 lost at both 512 KiB and 2 MiB.
    if (768 * 1024..=1024 * 1024).contains(&length) && std::is_x86_feature_detected!("avx2") {
        unsafe {
            avx2(source, destination, length);
        }
    } else {
        unsafe {
            core::ptr::copy_nonoverlapping(source, destination, length);
        }
    }
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn avx2(source: *const u8, destination: *mut u8, length: usize) {
    let mut cursor = 0;
    while length - cursor >= 128 {
        unsafe {
            let a = _mm256_loadu_si256(source.add(cursor).cast());
            let b = _mm256_loadu_si256(source.add(cursor + 32).cast());
            let c = _mm256_loadu_si256(source.add(cursor + 64).cast());
            let d = _mm256_loadu_si256(source.add(cursor + 96).cast());
            _mm256_storeu_si256(destination.add(cursor).cast(), a);
            _mm256_storeu_si256(destination.add(cursor + 32).cast(), b);
            _mm256_storeu_si256(destination.add(cursor + 64).cast(), c);
            _mm256_storeu_si256(destination.add(cursor + 96).cast(), d);
        }
        cursor += 128;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            source.add(cursor),
            destination.add(cursor),
            length - cursor,
        );
    }
    _mm256_zeroupper();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copies_unaligned_ranges_and_preserves_both_guards() {
        for length in [
            0,
            1,
            15,
            16,
            63,
            64,
            127,
            128,
            255,
            256,
            4095,
            4096,
            16385,
            512 * 1024,
            768 * 1024 - 1,
            768 * 1024,
            1024 * 1024,
            1024 * 1024 + 1,
            2 * 1024 * 1024,
            2 * 1024 * 1024 + 1,
        ] {
            let source: Vec<_> = (0..length + 32).map(|index| (index * 37) as u8).collect();
            let mut destination = vec![0xcc; length + 32];
            unsafe {
                copy(
                    source.as_ptr().add(3),
                    destination.as_mut_ptr().add(7),
                    length,
                );
            }
            assert_eq!(&destination[7..7 + length], &source[3..3 + length]);
            assert!(
                destination[..7]
                    .iter()
                    .chain(destination[7 + length..].iter())
                    .all(|byte| *byte == 0xcc)
            );
        }
    }
}
