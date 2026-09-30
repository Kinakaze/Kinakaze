//! Copy materialized shared pages once; zero only sparse holes. Coalesce
//! adjacent file pages whose backing slots are also adjacent.
use super::{BTreeMap, EIO, PAGE};

fn optimized() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_TMPFS_READ_OPT").is_none_or(|v| v != "0"))
}

/// # Safety
/// `page` returns at least PAGE readable bytes for each slot. Adjacent slots
/// refer to adjacent addresses, and the backing never overlaps `buffer`.
pub(super) unsafe fn read(
    pages: &BTreeMap<u64, u64>,
    offset: u64,
    buffer: &mut [u8],
    page: impl Fn(u64) -> Result<*mut u8, i32>,
) -> Result<(), i32> {
    if !optimized() {
        buffer.fill(0);
        let mut done = 0;
        while done < buffer.len() {
            let pos = offset + done as u64;
            let part = (PAGE - pos % PAGE).min((buffer.len() - done) as u64) as usize;
            if let Some(&slot) = pages.get(&(pos / PAGE)) {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        page(slot)?.add((pos % PAGE) as usize),
                        buffer.as_mut_ptr().add(done),
                        part,
                    )
                };
            }
            done += part;
        }
        return Ok(());
    }
    read_runs(pages, offset, buffer, page)
}

fn read_runs(
    pages: &BTreeMap<u64, u64>,
    offset: u64,
    buffer: &mut [u8],
    page: impl Fn(u64) -> Result<*mut u8, i32>,
) -> Result<(), i32> {
    if buffer.is_empty() {
        return Ok(());
    }
    let end = offset.checked_add(buffer.len() as u64).ok_or(EIO)?;
    let mut present = pages.range(offset / PAGE..=(end - 1) / PAGE).peekable();
    let mut done = 0;
    while let Some((&file_page, &slot)) = present.next() {
        let start = file_page.checked_mul(PAGE).ok_or(EIO)?.max(offset);
        let begin = (start - offset) as usize;
        buffer[done..begin].fill(0);
        let source = unsafe { page(slot)?.add((start % PAGE) as usize) };
        let mut last_page = file_page;
        let mut last_slot = slot;
        while present.peek().is_some_and(|&(&next_page, &next_slot)| {
            last_page.checked_add(1) == Some(next_page)
                && last_slot.checked_add(1) == Some(next_slot)
        }) {
            let (&next_page, &next_slot) = present.next().unwrap();
            // Validate every backing slot even when only one copy is emitted.
            let _ = page(next_slot)?;
            last_page = next_page;
            last_slot = next_slot;
        }
        let stop = last_page
            .checked_add(1)
            .and_then(|v| v.checked_mul(PAGE))
            .ok_or(EIO)?
            .min(end);
        let count = (stop - start) as usize;
        unsafe { std::ptr::copy_nonoverlapping(source, buffer.as_mut_ptr().add(begin), count) };
        done = begin + count;
    }
    buffer[done..].fill(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_runs_handle_unaligned_edges_holes_and_reordered_slots() {
        let backing: Vec<u8> = (0..5 * PAGE as usize)
            .map(|i| (i / PAGE as usize + 17) as u8)
            .collect();
        let pages = BTreeMap::from([(1, 0), (2, 1), (4, 4), (5, 2)]);
        for offset in [0, 7, PAGE - 1, PAGE, PAGE + 1, 3 * PAGE + 73] {
            for length in [0, 1, 29, PAGE as usize, 5 * PAGE as usize + 17] {
                let mut got = vec![0xcc; length];
                read_runs(&pages, offset, &mut got, |slot| {
                    if slot >= 5 {
                        return Err(EIO);
                    }
                    Ok(unsafe { backing.as_ptr().add(slot as usize * PAGE as usize) } as *mut u8)
                })
                .unwrap();
                for (i, byte) in got.into_iter().enumerate() {
                    let pos = offset + i as u64;
                    let expected = pages
                        .get(&(pos / PAGE))
                        .map_or(0, |slot| (*slot + 17) as u8);
                    assert_eq!(byte, expected, "offset={offset} len={length} i={i}");
                }
            }
        }
        let mut bytes = [0xcc; 4];
        assert_eq!(
            read_runs(&BTreeMap::from([(0, 9)]), 0, &mut bytes, |_| Err(EIO)),
            Err(EIO)
        );
    }

    #[test]
    #[ignore = "paired release memory I/O benchmark"]
    fn benchmark_shared_read() {
        let backing = vec![0xa7; 4 * 1024 * 1024];
        let pages = (0..backing.len() as u64 / PAGE).map(|i| (i, i)).collect();
        let mut output = vec![0; backing.len()];
        let began = std::time::Instant::now();
        for _ in 0..256 {
            unsafe {
                read(&pages, 0, &mut output, |slot| {
                    Ok(backing.as_ptr().add((slot * PAGE) as usize) as *mut u8)
                })
            }
            .unwrap();
            assert_eq!(output[0], 0xa7);
            assert_eq!(*output.last().unwrap(), 0xa7);
            std::hint::black_box(&output);
        }
        assert_eq!(output, backing);
        println!(
            "TMPFS_READ_BENCH {{\"optimized\":{},\"read_1gib_ns\":{}}}",
            optimized(),
            began.elapsed().as_nanos()
        );
    }
}
