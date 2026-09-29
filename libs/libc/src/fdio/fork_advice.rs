//! Page-granular fork policies, separate from native allocation boundaries.
//! The fork coordinator gets split ranges while the parent's views stay intact.
use super::*;
use kinakaze_runtime::ForkMappingBehavior as Behavior;
const OMIT: u8 = 1;
const ZERO: u8 = 2;
#[derive(Clone, Copy)]
struct Range {
    start: usize,
    end: usize,
    flags: u8,
}
static POLICIES: Mutex<Vec<Range>> = Mutex::new(Vec::new());
static PRESENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn partition(ranges: &[Range], start: usize, end: usize) -> Vec<Range> {
    let mut result = Vec::new();
    let mut cursor = start;
    for range in ranges.iter().filter(|r| r.start < end && r.end > start) {
        let from = range.start.max(start);
        if cursor < from {
            result.push(Range {
                start: cursor,
                end: from,
                flags: 0,
            });
        }
        cursor = range.end.min(end);
        result.push(Range {
            start: from,
            end: cursor,
            flags: range.flags,
        });
    }
    if cursor < end {
        result.push(Range {
            start: cursor,
            end,
            flags: 0,
        });
    }
    result
}
fn changed(ranges: &[Range], start: usize, end: usize, set: u8, clear: u8) -> Vec<Range> {
    let mut result = Vec::new();
    for &r in ranges {
        if r.start >= end || r.end <= start {
            result.push(r);
        } else {
            if r.start < start {
                result.push(Range { end: start, ..r });
            }
            if r.end > end {
                result.push(Range { start: end, ..r });
            }
        }
    }
    result.extend(
        partition(ranges, start, end)
            .into_iter()
            .filter_map(|mut r| {
                r.flags = (r.flags | set) & !clear;
                (r.flags != 0).then_some(r)
            }),
    );
    result.sort_unstable_by_key(|r| r.start);
    let mut merged: Vec<Range> = Vec::new();
    for r in result {
        if let Some(last) = merged.last_mut() {
            if last.end == r.start && last.flags == r.flags {
                last.end = r.end;
                continue;
            }
        }
        merged.push(r);
    }
    merged
}
pub(super) fn fragments(start: usize, mapping: &Mapping) -> Vec<(usize, Mapping, Behavior)> {
    let end = start + mapping.length;
    let spans = if PRESENT.load(Ordering::Acquire) {
        partition(
            &POLICIES.lock().unwrap_or_else(|e| e.into_inner()),
            start,
            end,
        )
    } else {
        vec![Range {
            start,
            end,
            flags: 0,
        }]
    };
    spans
        .into_iter()
        .map(|span| {
            let delta = span.start - start;
            let mut fragment = mapping.clone();
            fragment.length = span.end - span.start;
            if delta != 0 {
                fragment.base = span.start as *mut c_void;
            }
            fragment.backing_offset += delta as u64;
            let behavior = if span.flags & OMIT != 0 {
                Behavior::Omit
            } else if span.flags & ZERO != 0 {
                Behavior::Zero
            } else {
                Behavior::Copy
            };
            if behavior == Behavior::Zero {
                fragment.base = span.start as *mut c_void;
                fragment.kind = MappingKind::Reserved;
                fragment.backing = None;
                fragment.backing_offset = 0;
                fragment.view_protection = 0;
            }
            (span.start, fragment, behavior)
        })
        .collect()
}
pub(super) fn set(start: usize, end: usize, advice: i32) -> Result<(), i32> {
    validate_madvise_range(start, end, advice == 18)?;
    update(
        start,
        end,
        match advice {
            10 => OMIT,
            18 => ZERO,
            _ => 0,
        },
        match advice {
            11 => OMIT,
            19 => ZERO,
            _ => 0,
        },
    )
}
pub(super) fn clear(start: usize, length: usize) -> Result<(), i32> {
    if !PRESENT.load(Ordering::Acquire) {
        return Ok(());
    }
    let end = start
        .checked_add(page_rounded_length(length)?)
        .ok_or(EINVAL)?;
    update(start, end, 0, OMIT | ZERO)
}
pub(super) fn inherited_ranges(start: usize, end: usize) -> Vec<(usize, usize)> {
    partition(
        &POLICIES.lock().unwrap_or_else(|e| e.into_inner()),
        start,
        end,
    )
    .into_iter()
    .filter(|r| r.flags & OMIT == 0)
    .map(|r| (r.start, r.end))
    .collect()
}
fn update(start: usize, end: usize, set: u8, clear: u8) -> Result<(), i32> {
    let mut registry = mappings().lock().map_err(|_| EIO)?;
    // Anonymous snapshot conversion requires a whole native view. A policy
    // splitting that view uses the existing copied-section representation,
    // preserving dirty COW pages and leaving the parent's addresses intact.
    let snapshots: Vec<_> = registry
        .iter()
        .filter(|(base, m)| {
            **base < end
                && **base + m.length > start
                && (start > **base || end < **base + m.length)
                && m.backing.as_ref().is_some_and(BackingRef::is_snapshot)
        })
        .map(|(&base, m)| (base, m.clone()))
        .collect();
    for (base, mapping) in snapshots {
        cow_materialize::make_remappable(&mut registry, base, mapping)?;
    }
    let affected: Vec<_> = registry
        .iter()
        .filter(|(base, m)| **base < end && **base + m.length > start)
        .map(|(&base, m)| (base, m))
        .collect();
    for &(base, mapping) in &affected {
        unregister_fork_fragment(base, mapping);
    }
    {
        let mut policies = POLICIES.lock().map_err(|_| EIO)?;
        *policies = changed(&policies, start, end, set, clear);
        PRESENT.store(!policies.is_empty(), Ordering::Release);
    }
    for (base, mapping) in affected {
        register_fork_fragment(base, mapping);
    }
    Ok(())
}
unsafe extern "system" fn snapshot(buffer: *mut u8, capacity: usize) -> isize {
    let Ok(policies) = POLICIES.lock() else {
        return -(EIO as isize);
    };
    // DONTFORK VMAs are absent from the child. WIPEONFORK stays set there.
    let inherited: Vec<_> = policies.iter().filter(|r| r.flags & OMIT == 0).collect();
    let length = 8 + inherited.len() * 24;
    if buffer.is_null() {
        return length as isize;
    }
    if capacity < length {
        return -(ENOMEM as isize);
    }
    let output = unsafe { std::slice::from_raw_parts_mut(buffer, length) };
    output[..8].copy_from_slice(&(inherited.len() as u64).to_le_bytes());
    for (i, r) in inherited.into_iter().enumerate() {
        for (j, value) in [r.start as u64, r.end as u64, r.flags as u64]
            .into_iter()
            .enumerate()
        {
            output[8 + i * 24 + j * 8..16 + i * 24 + j * 8].copy_from_slice(&value.to_le_bytes());
        }
    }
    length as isize
}
unsafe extern "system" fn child(buffer: *const u8, length: usize) -> i32 {
    if buffer.is_null() || length < 8 {
        return EINVAL;
    }
    let input = unsafe { std::slice::from_raw_parts(buffer, length) };
    let count = u64::from_le_bytes(input[..8].try_into().unwrap()) as usize;
    if count.checked_mul(24).and_then(|v| v.checked_add(8)) != Some(length) {
        return EINVAL;
    }
    let mut next = Vec::new();
    for item in input[8..].chunks_exact(24) {
        let start = u64::from_le_bytes(item[..8].try_into().unwrap()) as usize;
        let end = u64::from_le_bytes(item[8..16].try_into().unwrap()) as usize;
        let flags = u64::from_le_bytes(item[16..24].try_into().unwrap());
        if start >= end || flags != ZERO as u64 {
            return EINVAL;
        }
        next.push(Range {
            start,
            end,
            flags: ZERO,
        });
    }
    let Ok(mut policies) = POLICIES.lock() else {
        return EIO;
    };
    *policies = next;
    PRESENT.store(!policies.is_empty(), Ordering::Release);
    0
}
extern "C" fn initialize() {
    assert!(kinakaze_runtime::register_fork_participant(
        kinakaze_runtime::ForkParticipant {
            abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
            priority: 29,
            key: 0x4b46_4f52_4b41_4431,
            prepare: None,
            snapshot: Some(snapshot),
            parent: None,
            child: Some(child),
        }
    ));
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = initialize;
