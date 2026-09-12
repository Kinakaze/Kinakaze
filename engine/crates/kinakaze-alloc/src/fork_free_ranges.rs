//! Allocation-free diagnostics for the frozen arena's large central free lists.
//!
//! These counters are deliberately not a copy-elision plan. They omit small
//! central blocks, local caches, alignment padding, and allocator headers.

use super::{
    ARENA_BASE, ARENA_MAGIC, ARENA_SIZE, ArenaFreezeGuard, BLOCK_ALLOCATED, BLOCK_MAGIC,
    BlockHeader, FREE_LIST_BIN_COUNT, HEADER_PAGE_SIZE, allocation_capacity, free_bin_index,
};
use core::mem::{align_of, size_of};

const LARGE_THRESHOLD: usize = 64 * 1024;

/// Exact counters for central free blocks with capacity strictly above 64 KiB.
///
/// Payload bytes include allocator size-class rounding, not just the original
/// allocation request. Whole pages are contained entirely inside each payload;
/// pages containing a block header are never counted. These are diagnostics,
/// not permission to discard or skip any arena bytes during fork.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForkFreeStats {
    pub large_central_payload_bytes: usize,
    pub large_central_whole_pages: usize,
    pub large_central_free_blocks: usize,
    pub scanned_nodes: usize,
}

/// A diagnostic scan fails closed instead of trusting corrupt free-list links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkFreeError {
    InvalidPageSize,
    InvalidBounds,
    UnreadableHeader,
    InvalidHeader,
    InvalidBin,
    Cycle,
    TraversalLimit,
    Overflow,
    Unsupported,
}

impl ArenaFreezeGuard {
    /// Inspects only large central free lists while this guard freezes them.
    ///
    /// This method does not allocate, modify allocator state, or inspect freed
    /// payload contents. Native header reads are fault-contained. Call only for
    /// explicit diagnostics and account for its cost separately from copying.
    /// Blocks at or below 64 KiB, including every local-cache block, are omitted.
    pub fn free_statistics(&self, page_size: usize) -> Result<ForkFreeStats, ForkFreeError> {
        if !page_size.is_power_of_two() {
            return Err(ForkFreeError::InvalidPageSize);
        }
        #[cfg(windows)]
        {
            // SAFETY: creating this guard initializes the header and owns the
            // arena, cache, and bin locks until this borrow ends.
            let arena = unsafe { &*super::header() };
            if arena.magic != ARENA_MAGIC {
                return Err(ForkFreeError::InvalidHeader);
            }
            scan(
                ARENA_BASE,
                arena.bump,
                arena.committed,
                &arena.free_heads,
                page_size,
                read_header,
            )
        }
        #[cfg(not(windows))]
        {
            Err(ForkFreeError::Unsupported)
        }
    }
}

#[cfg(windows)]
fn read_header(address: usize) -> Result<BlockHeader, ForkFreeError> {
    use core::ffi::c_void;
    use core::mem::MaybeUninit;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReadProcessMemory(
            process: *mut c_void,
            address: *const c_void,
            buffer: *mut c_void,
            size: usize,
            transferred: *mut usize,
        ) -> i32;
    }

    let mut value = MaybeUninit::<BlockHeader>::uninit();
    let mut transferred = 0;
    // Guest protection changes can make even an in-range header unreadable.
    // A raw Rust dereference here would turn diagnostics into a host AV.
    let success = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address as *const c_void,
            value.as_mut_ptr().cast(),
            size_of::<BlockHeader>(),
            &mut transferred,
        )
    };
    if success == 0 || transferred != size_of::<BlockHeader>() {
        return Err(ForkFreeError::UnreadableHeader);
    }
    // SAFETY: the full integer-only header was copied into local storage.
    Ok(unsafe { value.assume_init() })
}

fn scan(
    base: usize,
    used_len: usize,
    committed_len: usize,
    heads: &[usize; FREE_LIST_BIN_COUNT],
    page_size: usize,
    mut read: impl FnMut(usize) -> Result<BlockHeader, ForkFreeError>,
) -> Result<ForkFreeStats, ForkFreeError> {
    if !page_size.is_power_of_two() {
        return Err(ForkFreeError::InvalidPageSize);
    }
    if used_len < HEADER_PAGE_SIZE || used_len > committed_len || committed_len > ARENA_SIZE {
        return Err(ForkFreeError::InvalidBounds);
    }
    let data_start = base
        .checked_add(HEADER_PAGE_SIZE)
        .ok_or(ForkFreeError::Overflow)?;
    let end = base.checked_add(used_len).ok_or(ForkFreeError::Overflow)?;
    let available = used_len - HEADER_PAGE_SIZE;
    // Even a corrupt acyclic chain cannot monopolize a frozen process. Every
    // legitimate large allocation consumes at least this much arena space.
    let max_nodes = available / (LARGE_THRESHOLD + 1 + size_of::<BlockHeader>());
    let mut result = ForkFreeStats::default();
    for (bin, &head) in heads
        .iter()
        .enumerate()
        .skip(free_bin_index(LARGE_THRESHOLD + 1))
    {
        let mut current = head;
        // Brent's cycle detector uses fixed stack space and one header read
        // per visit. Across bins, canonical capacity validation rejects a node
        // linked into two different lists; within one list repetition is a cycle.
        let mut anchor = head;
        let mut power = 1usize;
        let mut distance = 0usize;
        while current != 0 {
            if result.scanned_nodes >= max_nodes {
                return Err(ForkFreeError::TraversalLimit);
            }
            if current < data_start || current % align_of::<BlockHeader>() != 0 {
                return Err(ForkFreeError::InvalidBounds);
            }
            let payload = current
                .checked_add(size_of::<BlockHeader>())
                .ok_or(ForkFreeError::Overflow)?;
            if payload > end {
                return Err(ForkFreeError::InvalidBounds);
            }
            let block = read(current)?;
            result.scanned_nodes += 1;
            if block.magic != BLOCK_MAGIC
                || block.next_free == BLOCK_ALLOCATED
                || block.capacity <= LARGE_THRESHOLD
                || allocation_capacity(block.capacity) != Some(block.capacity)
                || block.allocation_start < HEADER_PAGE_SIZE
                || block.allocation_start > current - base
            {
                return Err(ForkFreeError::InvalidHeader);
            }
            if free_bin_index(block.capacity) != bin {
                return Err(ForkFreeError::InvalidBin);
            }
            let payload_end = payload
                .checked_add(block.capacity)
                .ok_or(ForkFreeError::Overflow)?;
            if payload_end > end {
                return Err(ForkFreeError::InvalidBounds);
            }
            result.large_central_payload_bytes = result
                .large_central_payload_bytes
                .checked_add(block.capacity)
                .ok_or(ForkFreeError::Overflow)?;
            if result.large_central_payload_bytes > available {
                return Err(ForkFreeError::InvalidBounds);
            }
            let page_start = payload
                .checked_add(page_size - 1)
                .ok_or(ForkFreeError::Overflow)?
                & !(page_size - 1);
            let page_end = payload_end & !(page_size - 1);
            result.large_central_whole_pages = result
                .large_central_whole_pages
                .checked_add(page_end.saturating_sub(page_start) / page_size)
                .ok_or(ForkFreeError::Overflow)?;
            result.large_central_free_blocks += 1;

            current = block.next_free;
            if current != 0 {
                distance += 1;
                if current == anchor {
                    return Err(ForkFreeError::Cycle);
                }
                if distance == power {
                    anchor = current;
                    power = power.checked_mul(2).ok_or(ForkFreeError::Overflow)?;
                    distance = 0;
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: usize = 0x1000_0000;
    const CAPACITY: usize = 128 * 1024;
    const FIRST: usize = BASE + HEADER_PAGE_SIZE;
    const SECOND: usize = FIRST + size_of::<BlockHeader>() + CAPACITY;
    const USED: usize = SECOND + size_of::<BlockHeader>() + CAPACITY - BASE;

    fn block(address: usize, next_free: usize) -> BlockHeader {
        BlockHeader {
            magic: BLOCK_MAGIC,
            allocation_start: address - BASE,
            capacity: CAPACITY,
            next_free,
        }
    }

    #[test]
    fn large_central_payload_and_whole_pages_exclude_headers() {
        let mut heads = [0; FREE_LIST_BIN_COUNT];
        heads[free_bin_index(CAPACITY)] = FIRST;
        let result = scan(BASE, USED, USED, &heads, 4096, |address| {
            Ok(block(address, if address == FIRST { SECOND } else { 0 }))
        })
        .unwrap();
        assert_eq!(result.large_central_payload_bytes, 2 * CAPACITY);
        assert_eq!(result.large_central_whole_pages, 62);
        assert_eq!(result.large_central_free_blocks, 2);
        assert_eq!(result.scanned_nodes, 2);
    }

    #[test]
    fn cycles_wrong_bins_and_bad_ranges_are_bounded_errors() {
        let mut heads = [0; FREE_LIST_BIN_COUNT];
        heads[free_bin_index(CAPACITY)] = FIRST;
        assert_eq!(
            scan(BASE, USED, USED, &heads, 4096, |address| Ok(block(
                address, FIRST
            ))),
            Err(ForkFreeError::Cycle)
        );
        heads[free_bin_index(CAPACITY) + 1] = FIRST;
        assert_eq!(
            scan(BASE, USED, USED, &heads, 4096, |address| Ok(block(
                address, 0
            ))),
            Err(ForkFreeError::InvalidBin)
        );
        heads[free_bin_index(CAPACITY) + 1] = 0;
        assert_eq!(
            scan(BASE, USED, USED, &heads, 4096, |address| Ok(block(
                address, BASE
            ))),
            Err(ForkFreeError::InvalidBounds)
        );
        assert_eq!(
            scan(BASE, USED, USED, &heads, 3, |_| panic!("must not read")),
            Err(ForkFreeError::InvalidPageSize)
        );
        assert_eq!(
            scan(BASE, USED, USED - 1, &heads, 4096, |_| panic!(
                "must not read"
            )),
            Err(ForkFreeError::InvalidBounds)
        );
        assert_eq!(
            scan(BASE, USED, USED, &heads, 4096, |_| Err(
                ForkFreeError::UnreadableHeader
            )),
            Err(ForkFreeError::UnreadableHeader)
        );
    }

    #[cfg(windows)]
    #[test]
    fn frozen_native_statistics_count_freed_large_block() {
        let _test = super::super::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        super::super::initialize().unwrap();
        let allocation = unsafe { super::super::malloc(CAPACITY) };
        assert!(!allocation.is_null());
        // Keep a live successor so this exercises the central free-list scan.
        // A freed tail now returns directly to the bump cursor. This request is
        // larger than the used span, so it cannot reuse an earlier free block.
        let successor = unsafe { super::super::malloc((*super::super::header()).bump + 1) };
        assert!(!successor.is_null());
        let (guard, _) = super::super::freeze_snapshot().unwrap();
        let before = guard.free_statistics(4096);
        drop(guard);
        unsafe { super::super::free(allocation) };
        let (guard, _) = super::super::freeze_snapshot().unwrap();
        let after = guard.free_statistics(4096);
        drop(guard);
        let before = before.unwrap();
        let after = after.unwrap();
        assert_eq!(
            after.large_central_payload_bytes - before.large_central_payload_bytes,
            CAPACITY
        );
        assert_eq!(
            after.large_central_free_blocks - before.large_central_free_blocks,
            1
        );
        assert!(after.large_central_whole_pages > before.large_central_whole_pages);
        unsafe { super::super::free(successor) };
    }
}
