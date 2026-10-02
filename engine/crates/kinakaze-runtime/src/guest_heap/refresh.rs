use super::*;
use std::sync::OnceLock;
use windows_sys::Win32::System::Diagnostics::Debug::{FlushInstructionCache, ReadProcessMemory};

const INTERVAL: usize = 16;

#[derive(Default)]
pub(crate) struct RefreshStats {
    pub heap_bytes: usize,
    pub image_bytes: usize,
}

pub(crate) unsafe fn refresh_fork_snapshots(mappings: &[ForkMapping]) -> RefreshStats {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    static FORKS: AtomicUsize = AtomicUsize::new(0);
    if !*ENABLED.get_or_init(|| {
        std::env::var_os("KINAKAZE_FORK_HEAP_REFRESH").is_none_or(|value| value != "0")
    }) || FORKS.fetch_add(1, Ordering::Relaxed) % INTERVAL != INTERVAL - 1
    {
        return RefreshStats::default();
    }
    let Ok(registry) = crate::mappings().lock() else {
        return RefreshStats::default();
    };
    let candidates: Vec<_> = mappings
        .iter()
        .filter(|mapping| {
            (mapping.storage == ForkMappingStorage::RefreshableImage
                || (mapping.storage == ForkMappingStorage::AnonymousSnapshot
                    && mapping.base >= ARENA_BASE
                    && mapping
                        .base
                        .checked_add(mapping.len)
                        .is_some_and(|end| end <= ARENA_BASE + kinakaze_alloc::ARENA_SIZE)))
                && mapping.behavior == ForkMappingBehavior::Copy
                && mapping.domain == ForkMappingDomain::GuestMm
                && mapping.len >= 256 * 1024
                && mapping.len <= MAX_COMMIT_BYTES
                && mapping.backing_offset == 0
                && registry.handle_slots.contains(&mapping.backing_slot)
        })
        .copied()
        .collect();
    drop(registry);
    if candidates.is_empty() {
        return RefreshStats::default();
    }
    let Some(_heap) = kinakaze_alloc::guest::freeze_if_initialized() else {
        return RefreshStats::default();
    };
    let Ok(_threads) = (unsafe { crate::freeze_memory_threads() }) else {
        return RefreshStats::default();
    };
    let mut refreshed = RefreshStats::default();
    for mapping in &candidates {
        let slot = mapping.backing_slot as *const AtomicUsize;
        let original = unsafe { (*slot).load(Ordering::Acquire) } as HANDLE;
        let Some(mut snapshot) = (unsafe { Snapshot::capture(mapping) }) else {
            continue;
        };
        if unsafe { snapshot.replace(mapping, original, snapshot.section) } {
            unsafe { (*slot).store(snapshot.section as usize, Ordering::Release) };
            snapshot.section = ptr::null_mut();
            unsafe { CloseHandle(original) };
            if mapping.storage == ForkMappingStorage::RefreshableImage {
                refreshed.image_bytes += mapping.len;
            } else {
                refreshed.heap_bytes += mapping.len;
            }
        }
    }
    refreshed
}

#[derive(Clone, Copy)]
struct Run {
    offset: usize,
    len: usize,
    protection: u32,
}

struct Snapshot {
    section: HANDLE,
    staging: MEMORY_MAPPED_VIEW_ADDRESS,
    runs: *mut Run,
    count: usize,
    allocation: u32,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        unsafe {
            if !self.staging.Value.is_null() {
                UnmapViewOfFile(self.staging);
            }
            if !self.section.is_null() {
                CloseHandle(self.section);
            }
            if !self.runs.is_null() {
                VirtualFree(self.runs.cast(), 0, MEM_RELEASE);
            }
        }
    }
}

unsafe fn query(address: usize) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut info = MEMORY_BASIC_INFORMATION::default();
    (unsafe { VirtualQuery(address as _, &mut info, size_of_val(&info)) } != 0).then_some(info)
}

impl Snapshot {
    unsafe fn capture(mapping: &ForkMapping) -> Option<Self> {
        let end = mapping.base.checked_add(mapping.len)?;
        let stack = 0u8;
        if mapping.len == 0 || (mapping.base..end).contains(&(ptr::addr_of!(stack) as usize)) {
            return None;
        }
        let first = unsafe { query(mapping.base)? };
        if !matches!(
            first.AllocationProtect & 0xff,
            PAGE_WRITECOPY | PAGE_EXECUTE_WRITECOPY
        ) {
            return None;
        }
        let mut cursor = mapping.base;
        let mut count = 0usize;
        while cursor < end {
            let info = unsafe { query(cursor)? };
            let next = (info.BaseAddress as usize).checked_add(info.RegionSize)?;
            if info.AllocationBase as usize != mapping.base
                || info.State != MEM_COMMIT
                || info.Type != MEM_MAPPED
                || next <= cursor
                || next > end
            {
                return None;
            }
            count += 1;
            cursor = next;
        }
        if unsafe { query(end)? }.AllocationBase as usize == mapping.base {
            return None;
        }
        let runs = unsafe {
            VirtualAlloc(
                ptr::null(),
                count.checked_mul(size_of::<Run>())?,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        }
        .cast::<Run>();
        if runs.is_null() {
            return None;
        }
        let mut snapshot = Self {
            section: ptr::null_mut(),
            staging: MEMORY_MAPPED_VIEW_ADDRESS {
                Value: ptr::null_mut(),
            },
            runs,
            count,
            allocation: first.AllocationProtect,
        };
        snapshot.section = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_EXECUTE_READWRITE,
                (mapping.len as u64 >> 32) as u32,
                mapping.len as u32,
                ptr::null(),
            )
        };
        if snapshot.section.is_null() {
            return None;
        }
        snapshot.staging =
            unsafe { MapViewOfFile(snapshot.section, FILE_MAP_WRITE, 0, 0, mapping.len) };
        if snapshot.staging.Value.is_null() {
            return None;
        }
        cursor = mapping.base;
        for index in 0..count {
            let info = unsafe { query(cursor)? };
            let offset = cursor - mapping.base;
            let len = info.RegionSize;
            unsafe {
                runs.add(index).write(Run {
                    offset,
                    len,
                    protection: info.Protect,
                })
            };
            let changed = info.Protect & PAGE_GUARD != 0
                || matches!(info.Protect & 0xff, PAGE_NOACCESS | PAGE_EXECUTE);
            let mut previous = 0;
            if changed
                && unsafe { VirtualProtect(cursor as _, len, PAGE_READONLY, &mut previous) } == 0
            {
                return None;
            }
            let mut read = 0;
            let copied = unsafe {
                ReadProcessMemory(
                    GetCurrentProcess(),
                    cursor as _,
                    snapshot.staging.Value.byte_add(offset),
                    len,
                    &mut read,
                )
            } != 0
                && read == len;
            let restore =
                crate::memory_protection::protection_for_allocation(snapshot.allocation, previous);
            if changed && unsafe { VirtualProtect(cursor as _, len, restore, &mut previous) } == 0 {
                std::process::abort();
            }
            if !copied {
                return None;
            }
            cursor += len;
        }
        Some(snapshot)
    }

    unsafe fn protect(&self, base: usize) -> bool {
        for index in 0..self.count {
            let run = unsafe { *self.runs.add(index) };
            let mut previous = 0;
            let protection = crate::memory_protection::protection_for_allocation(
                self.allocation,
                run.protection,
            );
            if unsafe {
                VirtualProtect((base + run.offset) as _, run.len, protection, &mut previous)
            } == 0
            {
                return false;
            }
        }
        true
    }

    unsafe fn replace(&self, mapping: &ForkMapping, original: HANDLE, replacement: HANDLE) -> bool {
        let process = unsafe { GetCurrentProcess() };
        let current = MEMORY_MAPPED_VIEW_ADDRESS {
            Value: mapping.base as _,
        };
        if unsafe { UnmapViewOfFile2(process, current, MEM_PRESERVE_PLACEHOLDER) } == 0 {
            return false;
        }
        let view = unsafe {
            MapViewOfFile3(
                replacement,
                process,
                mapping.base as _,
                0,
                mapping.len,
                MEM_REPLACE_PLACEHOLDER,
                self.allocation,
                ptr::null_mut(),
                0,
            )
        };
        if view.Value as usize == mapping.base
            && unsafe { self.protect(mapping.base) }
            && unsafe { FlushInstructionCache(process, mapping.base as _, mapping.len) } != 0
        {
            return true;
        }
        if !view.Value.is_null()
            && unsafe { UnmapViewOfFile2(process, view, MEM_PRESERVE_PLACEHOLDER) } == 0
        {
            std::process::abort();
        }
        let restored = unsafe {
            MapViewOfFile3(
                original,
                process,
                mapping.base as _,
                0,
                mapping.len,
                MEM_REPLACE_PLACEHOLDER,
                self.allocation,
                ptr::null_mut(),
                0,
            )
        };
        if restored.Value as usize != mapping.base {
            std::process::abort();
        }
        unsafe {
            ptr::copy_nonoverlapping(
                self.staging.Value.cast::<u8>(),
                restored.Value.cast(),
                mapping.len,
            )
        };
        if !unsafe { self.protect(mapping.base) }
            || unsafe { FlushInstructionCache(process, mapping.base as _, mapping.len) } == 0
        {
            std::process::abort();
        }
        false
    }
}

#[cfg(test)]
mod tests;
