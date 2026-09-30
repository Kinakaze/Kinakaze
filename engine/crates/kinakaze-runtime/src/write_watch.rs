//! Copy hints for explicitly registered, fresh zero-filled private allocations.
//!
//! Provenance is process-local. Fresh fork destinations rebuild it from their
//! own cumulative write-watch history, including the parent's native copies.
//! Missing or invalid history always falls back to the full private copier.

use crate::{ForkMapping, ForkMappingBehavior, ForkMappingStorage};
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::System::Memory::{
    GetWriteWatch, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, VirtualQuery,
};
use windows_sys::Win32::System::SystemServices::MEM_WRITE_WATCH;

const PAGE: usize = 4096;
// A hint must not allocate unbounded scratch for a large sparse reservation.
const MAX_REGION: usize = 64 * 1024 * 1024;
static ENABLED: OnceLock<bool> = OnceLock::new();
static CHILDREN_ENABLED: OnceLock<bool> = OnceLock::new();

fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_FORK_WRITE_WATCH").is_none_or(|v| v != "0"))
}

fn children_enabled() -> bool {
    *CHILDREN_ENABLED.get_or_init(|| {
        std::env::var_os("KINAKAZE_FORK_WRITE_WATCH_CHILDREN").is_none_or(|v| v != "0")
    })
}

/// Prepared before the fork freeze; never perform lazy environment work here.
pub(crate) fn allocation_flags() -> u32 {
    if ENABLED.get().copied().unwrap_or(false) && CHILDREN_ENABLED.get().copied().unwrap_or(false) {
        MEM_WRITE_WATCH
    } else {
        0
    }
}

fn origins() -> &'static Mutex<Vec<(usize, usize)>> {
    static ORIGINS: OnceLock<Mutex<Vec<(usize, usize)>>> = OnceLock::new();
    ORIGINS.get_or_init(|| Mutex::new(Vec::new()))
}

// All registry access follows the shared mapping transaction. No registry lock
// or allocation is allowed after sibling threads/the allocator are frozen.
pub(crate) fn invalidate(base: usize, len: usize) {
    origins()
        .lock()
        .unwrap()
        .retain(|&(start, size)| start + size <= base || base.saturating_add(len) <= start);
}

pub(crate) fn remove(base: usize) {
    origins()
        .lock()
        .unwrap()
        .retain(|&(start, size)| base < start || base >= start + size);
}

pub(crate) fn register(mapping: ForkMapping) {
    if mapping.len > MAX_REGION {
        return;
    }
    let mut origins = origins().lock().unwrap();
    if !origins.contains(&(mapping.base, mapping.len)) {
        origins.push((mapping.base, mapping.len));
    }
}

/// Runs only after all child participants have adopted their allocator/TLS.
///
/// # Safety
/// These entries must describe the fork coordinator's freshly allocated child
/// destinations, not arbitrary imported memory. Copying into them must use
/// native writes that enter cumulative history. No caller may reset history.
pub(crate) unsafe fn adopt_fork_destinations(mappings: &[kinakaze_alloc::SharedForkMapping]) {
    if !enabled() || !children_enabled() {
        return;
    }
    let Some(_transaction) = crate::begin_fork_mapping_transaction() else {
        return;
    };
    for mapping in mappings {
        if mapping.storage != ForkMappingStorage::Ordinary as u32
            || mapping.behavior != ForkMappingBehavior::Copy as u32
        {
            continue;
        }
        let Some(domain) = crate::ForkMappingDomain::from_raw(mapping.domain) else {
            continue;
        };
        unsafe {
            adopt_destination(ForkMapping {
                base: mapping.base,
                len: mapping.len,
                behavior: ForkMappingBehavior::Copy,
                storage: ForkMappingStorage::Ordinary,
                backing_slot: mapping.backing_slot,
                backing_offset: mapping.backing_offset,
                view_protection: mapping.view_protection,
                domain,
            })
        };
    }
}

unsafe fn adopt_destination(mapping: ForkMapping) -> bool {
    if mapping.storage != ForkMappingStorage::Ordinary
        || mapping.behavior != ForkMappingBehavior::Copy
        || mapping.len == 0
        || mapping.len > MAX_REGION
        || mapping.base % PAGE != 0
        || mapping.len % PAGE != 0
        || mapping.base.checked_add(mapping.len).is_none()
    {
        return false;
    }
    let mut region = MEMORY_BASIC_INFORMATION::default();
    if unsafe {
        VirtualQuery(
            mapping.base as _,
            &mut region,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    } == 0
        || region.Type != MEM_PRIVATE
    {
        return false;
    }
    let mut plan = Plan {
        base: mapping.base,
        len: mapping.len,
        pages: vec![0; mapping.len / PAGE],
        used: None,
    };
    if !unsafe { plan.capture() } {
        return false;
    }
    register(mapping);
    true
}

pub(crate) struct Plan {
    pub base: usize,
    len: usize,
    pages: Vec<usize>,
    used: Option<usize>,
}

pub(crate) fn prepare(mappings: &[ForkMapping]) -> Vec<Plan> {
    let enabled = enabled();
    let _ = children_enabled();
    if !enabled {
        return Vec::new();
    }
    let origins = origins().lock().unwrap();
    mappings
        .iter()
        .filter_map(|mapping| {
            (mapping.storage == ForkMappingStorage::Ordinary
                && mapping.behavior == ForkMappingBehavior::Copy
                && origins.contains(&(mapping.base, mapping.len)))
            .then(|| Plan {
                base: mapping.base,
                len: mapping.len,
                pages: vec![0; mapping.len / PAGE],
                used: None,
            })
        })
        .collect()
}

impl Plan {
    /// Query once, after the parent freeze, without resetting cumulative
    /// history. Even reads can conservatively dirty demand-zero pages on some
    /// Windows versions; extra pages are harmless. Never infer zero from a
    /// working-set query or from the stack pointer.
    pub(crate) unsafe fn capture(&mut self) -> bool {
        let mut count = self.pages.len();
        let mut granularity = 0;
        self.used = None;
        if unsafe {
            GetWriteWatch(
                0,
                self.base as _,
                self.len,
                self.pages.as_mut_ptr().cast(),
                &mut count,
                &mut granularity,
            )
        } != 0
            || granularity as usize != PAGE
            || count > self.pages.len()
        {
            return false;
        }
        let end = self.base + self.len;
        let pages = &mut self.pages[..count];
        if pages
            .iter()
            .any(|&address| address < self.base || address >= end || address % PAGE != 0)
        {
            return false;
        }
        pages.sort_unstable();
        self.used = Some(count);
        true
    }

    /// Emit complete consecutive dirty pages within one committed protection
    /// run. Destination bytes must be fresh zero bytes. The caller restores
    /// protection for this run even if the copy callback fails.
    pub(crate) fn copy<E>(
        &self,
        base: usize,
        len: usize,
        mut copy: impl FnMut(usize, usize) -> Result<(), E>,
    ) -> Result<(usize, usize), E> {
        let pages = &self.pages[..self.used.expect("captured write-watch plan")];
        let end = base + len;
        let first = pages.partition_point(|&page| page < base);
        let last = pages.partition_point(|&page| page < end);
        let mut run: Option<(usize, usize)> = None;
        let (mut copied, mut calls) = (0, 0);
        for &page in &pages[first..last] {
            if let Some((start, limit)) = run {
                if page <= limit {
                    run = Some((start, limit.max(page + PAGE)));
                    continue;
                }
                copy(start, limit - start)?;
                copied += limit - start;
                calls += 1;
            }
            run = Some((page, page + PAGE));
        }
        if let Some((start, limit)) = run {
            copy(start, limit - start)?;
            copied += limit - start;
            calls += 1;
        }
        Ok((copied, calls))
    }
}

#[cfg(test)]
mod tests;
