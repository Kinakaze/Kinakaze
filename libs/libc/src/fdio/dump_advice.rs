//! MADV_DONTDUMP/DODUMP applied to Windows Error Reporting memory dumps.
//! Keep exact page intervals, split overlapping changes, retire them on unmap,
//! and recreate the exclusions after fork in the child's own WER process state.
use super::*;
use std::collections::BTreeMap;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn WerRegisterExcludedMemoryBlock(address: *const c_void, length: u32) -> i32;
    fn WerUnregisterExcludedMemoryBlock(address: *const c_void) -> i32;
}
type Ranges = BTreeMap<usize, usize>;
static EXCLUDED: Mutex<Ranges> = Mutex::new(BTreeMap::new());
static PRESENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const KEY: u64 = 0x4b44_554d_5056_3031;

fn changed(current: &Ranges, start: usize, end: usize, exclude: bool) -> Ranges {
    let mut next = Ranges::new();
    for (&from, &to) in current {
        if from >= end || to <= start {
            next.insert(from, to);
        } else {
            if from < start {
                next.insert(from, start);
            }
            if to > end {
                next.insert(end, to);
            }
        }
    }
    if exclude {
        next.insert(start, end);
    }
    let mut merged = Ranges::new();
    for (from, to) in next {
        if let Some((&previous, &limit)) = merged.last_key_value() {
            if from <= limit {
                merged.insert(previous, limit.max(to));
                continue;
            }
        }
        merged.insert(from, to);
    }
    merged
}

// WER accepts ULONG lengths. Splitting large VMAs must not truncate the range.
fn native_ranges(ranges: &Ranges) -> Vec<(usize, u32)> {
    let mut result = Vec::new();
    for (&start, &end) in ranges {
        let mut cursor = start;
        while cursor < end {
            let length = (end - cursor).min((u32::MAX as usize) & !4095);
            result.push((cursor, length as u32));
            cursor += length;
        }
    }
    result
}
fn register((start, length): (usize, u32)) -> Result<(), i32> {
    let result = unsafe { WerRegisterExcludedMemoryBlock(start as *const _, length) };
    if result >= 0 {
        Ok(())
    } else {
        Err(if result as u32 == 0x8007_000e {
            ENOMEM
        } else {
            EINVAL
        })
    }
}
fn unregister((start, _): (usize, u32)) -> Result<(), i32> {
    if unsafe { WerUnregisterExcludedMemoryBlock(start as *const _) } >= 0 {
        Ok(())
    } else {
        Err(EIO)
    }
}
fn apply(current: &Ranges, next: &Ranges) -> Result<(), i32> {
    let old = native_ranges(current);
    let new = native_ranges(next);
    let mut removed = Vec::new();
    let mut added = Vec::new();
    let result = (|| {
        for &range in old.iter().filter(|range| !new.contains(range)) {
            unregister(range)?;
            removed.push(range);
        }
        for &range in new.iter().filter(|range| !old.contains(range)) {
            register(range)?;
            added.push(range);
        }
        Ok(())
    })();
    if result.is_err() {
        // Never report a recoverable failure while silently losing an earlier
        // exclusion. Roll back the actual OS registration as well as metadata.
        for range in added {
            if unregister(range).is_err() {
                std::process::abort();
            }
        }
        for range in removed {
            if register(range).is_err() {
                std::process::abort();
            }
        }
    }
    result
}
pub(super) fn set(start: usize, end: usize, exclude: bool) -> Result<(), i32> {
    let mut ranges = EXCLUDED.lock().map_err(|_| EIO)?;
    let next = changed(&ranges, start, end, exclude);
    apply(&ranges, &next)?;
    *ranges = next;
    PRESENT.store(!ranges.is_empty(), Ordering::Release);
    Ok(())
}
pub(super) fn clear(start: usize, length: usize) -> Result<(), i32> {
    if !PRESENT.load(Ordering::Acquire) {
        return Ok(());
    }
    let end = start
        .checked_add(page_rounded_length(length)?)
        .ok_or(EINVAL)?;
    set(start, end, false)
}

unsafe extern "system" fn snapshot(buffer: *mut u8, capacity: usize) -> isize {
    let Ok(ranges) = EXCLUDED.lock() else {
        return -(EIO as isize);
    };
    let ranges: Ranges = ranges
        .iter()
        .flat_map(|(&start, &end)| fork_advice::inherited_ranges(start, end))
        .collect();
    let length = 8 + ranges.len() * 16;
    if buffer.is_null() {
        return length as isize;
    }
    if capacity < length {
        return -(ENOMEM as isize);
    }
    let output = unsafe { std::slice::from_raw_parts_mut(buffer, length) };
    output[..8].copy_from_slice(&(ranges.len() as u64).to_le_bytes());
    for (index, (&start, &end)) in ranges.iter().enumerate() {
        output[8 + index * 16..16 + index * 16].copy_from_slice(&(start as u64).to_le_bytes());
        output[16 + index * 16..24 + index * 16].copy_from_slice(&(end as u64).to_le_bytes());
    }
    length as isize
}
unsafe extern "system" fn child(buffer: *const u8, length: usize) -> i32 {
    if buffer.is_null() || length < 8 {
        return EINVAL;
    }
    let input = unsafe { std::slice::from_raw_parts(buffer, length) };
    let count = u64::from_le_bytes(input[..8].try_into().unwrap()) as usize;
    if count.checked_mul(16).and_then(|n| n.checked_add(8)) != Some(length) {
        return EINVAL;
    }
    let mut next = Ranges::new();
    for chunk in input[8..].chunks_exact(16) {
        let start = u64::from_le_bytes(chunk[..8].try_into().unwrap()) as usize;
        let end = u64::from_le_bytes(chunk[8..].try_into().unwrap()) as usize;
        if start >= end || start % 4096 != 0 || end % 4096 != 0 {
            return EINVAL;
        }
        next.insert(start, end);
    }
    let Ok(mut current) = EXCLUDED.lock() else {
        return EIO;
    };
    if let Err(error) = apply(&current, &next) {
        return error;
    }
    *current = next;
    PRESENT.store(!current.is_empty(), Ordering::Release);
    0
}
extern "C" fn initialize() {
    let registered = unsafe {
        kinakaze_runtime::register_fork_participant_without_inherited_handles(
            kinakaze_runtime::ForkParticipant {
                abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
                priority: 40,
                key: KEY,
                prepare: None,
                snapshot: Some(snapshot),
                parent: None,
                child: Some(child),
            },
        )
    };
    assert!(registered, "memory dump fork participant");
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = initialize;

#[test]
fn overlapping_dump_policy_splits_and_rejoins_real_wer_ranges() {
    let base = map(
        std::ptr::null_mut(),
        4 * 4096,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    )
    .unwrap();
    let start = base as usize;
    set(start, start + 4 * 4096, true).unwrap();
    set(start + 4096, start + 3 * 4096, false).unwrap();
    {
        let ranges = EXCLUDED.lock().unwrap();
        assert_eq!(ranges.get(&start), Some(&(start + 4096)));
        assert_eq!(ranges.get(&(start + 3 * 4096)), Some(&(start + 4 * 4096)));
    }
    set(start + 4096, start + 3 * 4096, true).unwrap();
    assert_eq!(
        EXCLUDED.lock().unwrap().get(&start),
        Some(&(start + 4 * 4096))
    );
    clear(start, 4 * 4096).unwrap();
    unmap(base, 4 * 4096).unwrap();
}
