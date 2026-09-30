//! Copy into fresh zero-initialized storage without faulting zero pages in the
//! child. This is only valid for a frozen, readable private source and a new
//! destination: retained sections and existing child memory must use full copy.
use core::arch::x86_64::*;

// SSE2 is part of the Windows x64 baseline; no lazy feature detection or
// allocation is allowed while the fork coordinator has frozen the allocator.
unsafe fn zero(bytes: *const u8, length: usize) -> bool {
    // Raw loads also cover active stacks: creating a Rust slice spanning this
    // function's own writable frames would assert an invalid shared borrow.
    let mut offset = 0;
    while length - offset >= 64 {
        let p = unsafe { bytes.add(offset) };
        let value = unsafe {
            _mm_or_si128(
                _mm_or_si128(_mm_loadu_si128(p.cast()), _mm_loadu_si128(p.add(16).cast())),
                _mm_or_si128(
                    _mm_loadu_si128(p.add(32).cast()),
                    _mm_loadu_si128(p.add(48).cast()),
                ),
            )
        };
        if unsafe { _mm_movemask_epi8(_mm_cmpeq_epi8(value, _mm_setzero_si128())) } != 0xffff {
            return false;
        }
        offset += 64;
    }
    while offset < length {
        if unsafe { bytes.add(offset).read() } != 0 {
            return false;
        }
        offset += 1;
    }
    true
}

/// `source` must remain readable and unchanged for `length` bytes throughout
/// this call. `copy` writes the indicated relative range into fresh zero pages.
/// Trim zero pages at run boundaries, coalescing short gaps so sparse random
/// data cannot turn each populated page into a remote-memory syscall.
pub(super) unsafe fn copy<E>(
    source: usize,
    length: usize,
    mut copy: impl FnMut(usize, usize) -> Result<(), E>,
) -> Result<(usize, usize), E> {
    const BLOCK: usize = 64 * 1024;
    const PAGE: usize = 4096;
    let mut start = None;
    let mut end = 0;
    let mut offset = 0;
    let mut copied = 0;
    let mut calls = 0;
    while offset < length {
        let len = BLOCK.min(length - offset);
        if !unsafe { zero((source + offset) as *const u8, len) } {
            let mut first = 0;
            while first + PAGE <= len && unsafe { zero((source + offset + first) as _, PAGE) } {
                first += PAGE;
            }
            let mut last = len;
            while last > first {
                let page = (last - 1) / PAGE * PAGE;
                if !unsafe { zero((source + offset + page) as _, last - page) } {
                    break;
                }
                last = page;
            }
            let begin = offset + first;
            if start.is_some() && begin - end >= 2 * BLOCK {
                let previous = start.take().unwrap();
                copy(previous, end - previous)?;
                copied += end - previous;
                calls += 1;
            }
            start.get_or_insert(begin);
            end = offset + last;
        }
        offset += len;
    }
    if let Some(begin) = start {
        copy(begin, end - begin)?;
        copied += end - begin;
        calls += 1;
    }
    Ok((copied, calls))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_dense_unaligned_and_tail_bytes_survive() {
        for length in [0, 1, 63, 64, 65, 65535, 65536, 65537, 8 * 1024 * 1024 + 17] {
            let mut data = vec![0u8; length + 1];
            let source = &mut data[1..];
            for dense in [false, true] {
                if dense {
                    source.fill(0xab);
                } else if length != 0 {
                    source[0] = 0x81;
                    source[length - 1] = 0xff;
                }
                let mut target = vec![0; length];
                let stats = unsafe {
                    copy(source.as_ptr() as usize, length, |offset, len| {
                        target[offset..offset + len].copy_from_slice(&source[offset..offset + len]);
                        Ok::<_, ()>(())
                    })
                }
                .unwrap();
                assert!(source == target, "length={length}, dense={dense}");
                if dense {
                    assert_eq!(stats, (length, usize::from(length != 0)));
                } else if length > 128 * 1024 {
                    assert!(stats.0 <= 128 * 1024);
                }
            }
        }
        // Test every SIMD lane, including bytes with a clear high bit.
        for index in 0..129 {
            let mut bytes = [0; 129];
            bytes[index] = 1;
            assert!(!unsafe { zero(bytes.as_ptr(), bytes.len()) });
        }
    }

    #[test]
    fn zero_runs_need_no_writes_and_errors_stop_copying() {
        let mut source = vec![0u8; 256 * 1024];
        assert_eq!(
            unsafe {
                copy(source.as_ptr() as usize, source.len(), |_, _| {
                    Err::<(), _>(7)
                })
            },
            Ok((0, 0))
        );
        source[0] = 1;
        source[128 * 1024] = 2;
        let mut calls = 0;
        assert_eq!(
            unsafe {
                copy(source.as_ptr() as usize, source.len(), |offset, _| {
                    calls += 1;
                    assert_eq!(offset, 0);
                    Err::<(), _>(7)
                })
            },
            Err(7)
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn sparse_edges_trim_pages_without_amplifying_scattered_writes() {
        let mut source = vec![0u8; 4 * 1024 * 1024];
        let mut target = vec![0u8; source.len()];
        source[4096 + 7] = 0x31;
        let last = source.len() - 4096 - 1;
        source[last] = 0xe2;
        let stats = unsafe {
            copy(source.as_ptr() as usize, source.len(), |offset, len| {
                target[offset..offset + len].copy_from_slice(&source[offset..offset + len]);
                Ok::<_, ()>(())
            })
        }
        .unwrap();
        assert_eq!(source, target);
        assert_eq!(stats, (8192, 2));

        // One byte in every 64 KiB block still requires just one remote copy.
        source.fill(0);
        target.fill(0);
        for index in (17..source.len()).step_by(64 * 1024) {
            source[index] = 1;
        }
        let stats = unsafe {
            copy(source.as_ptr() as usize, source.len(), |offset, len| {
                target[offset..offset + len].copy_from_slice(&source[offset..offset + len]);
                Ok::<_, ()>(())
            })
        }
        .unwrap();
        assert_eq!(source, target);
        assert_eq!(stats.1, 1);
        assert_eq!(stats.0, source.len() - 60 * 1024);
    }
}
