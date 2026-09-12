//! C allocation storage, separate from loader Rust objects and native handles.
//! The allocator front end is shared with ManagedAllocator, its state is not.
use super::*;

pub const ARENA_BASE: usize = 0x0000_5000_0000_0000;
pub const HEADER_BYTES: usize = size_of::<ArenaHeader>().next_power_of_two();
pub const MAX_COMMIT_BYTES: usize = 16 * 1024 * 1024;
const INITIAL_COMMIT: usize = 1024 * 1024;
const MAGIC: u64 = 0x4352_5953_4748_5031; // guest heap, layout 1
static STATE: AtomicU32 = AtomicU32::new(ARENA_UNCHECKED);

pub(super) fn release_thread_cache(shard: usize) {
    #[cfg(windows)]
    if STATE.load(Ordering::Acquire) == ARENA_READY {
        unsafe { drain_local_cache(ARENA_BASE as *mut ArenaHeader, shard) };
    }
    #[cfg(not(windows))]
    let _ = shard;
}

pub struct GuestAllocator;
unsafe impl GlobalAlloc for GuestAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if initialize().is_err() {
            return ptr::null_mut();
        }
        #[cfg(windows)]
        return unsafe { allocate_from(ARENA_BASE as *mut ArenaHeader, layout) };
        #[cfg(not(windows))]
        unsafe {
            super::allocate(layout)
        }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
        unsafe { free(pointer) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        unsafe { reallocate(pointer, layout.align(), size) }
    }
}

pub fn contains(address: usize) -> bool {
    #[cfg(windows)]
    return (ARENA_BASE + HEADER_BYTES..ARENA_BASE + super::ARENA_SIZE).contains(&address);
    #[cfg(not(windows))]
    {
        let _ = address;
        false
    }
}

#[derive(Default)]
pub struct Statistics {
    pub arena: usize,
    pub free_blocks: usize,
    pub used: usize,
    pub free: usize,
    pub tail: usize,
}

/// Walk free lists only when queried. An unused allocator stays uninitialized.
#[cfg(windows)]
pub fn statistics() -> Statistics {
    let Some(_guard) = freeze_if_initialized() else {
        return Statistics::default();
    };
    let arena = unsafe { &*(ARENA_BASE as *const ArenaHeader) };
    let mut result = Statistics {
        arena: arena.committed.saturating_sub(HEADER_BYTES),
        tail: arena.committed.saturating_sub(arena.bump),
        ..Statistics::default()
    };
    result.free = result.tail;
    for &first in arena.free_heads.iter().chain(
        arena
            .local_caches
            .iter()
            .flat_map(|cache| cache.heads.iter()),
    ) {
        let mut next = first;
        while next != 0 {
            let block = unsafe { &*(next as *const BlockHeader) };
            result.free_blocks += 1;
            result.free += next - ARENA_BASE + size_of::<BlockHeader>() + block.capacity
                - block.allocation_start;
            next = block.next_free;
        }
    }
    result.used = result.arena.saturating_sub(result.free);
    result
}

/// Same allocation contract as the host C allocator, in guest-owned storage.
pub unsafe fn malloc(size: usize) -> *mut u8 {
    unsafe { memalign(16, size) }
}
pub unsafe fn memalign(align: usize, size: usize) -> *mut u8 {
    let Ok(layout) = Layout::from_size_align(size.max(1), align.max(align_of::<usize>())) else {
        return ptr::null_mut();
    };
    unsafe { GuestAllocator.alloc(layout) }
}
/// Accepts legacy host-owned C buffers as well, without mixing the free lists.
pub unsafe fn free(pointer: *mut u8) {
    if pointer.is_null() {
        return;
    }
    if contains(pointer as usize) {
        if initialize().is_ok() {
            unsafe { deallocate_from(ARENA_BASE as _, pointer) };
        }
    } else {
        unsafe { super::free(pointer) };
    }
}
pub unsafe fn usable_size(pointer: *mut u8) -> usize {
    if pointer.is_null() {
        return 0;
    }
    if contains(pointer as usize) {
        unsafe { (*block_header(pointer)).capacity }
    } else {
        unsafe { super::usable_size(pointer) }
    }
}
pub unsafe fn reallocate(pointer: *mut u8, align: usize, size: usize) -> *mut u8 {
    if pointer.is_null() {
        return unsafe { memalign(align, size) };
    }
    if size == 0 {
        unsafe { free(pointer) };
        return ptr::null_mut();
    }
    let capacity = unsafe { usable_size(pointer) };
    if size <= capacity {
        return pointer;
    }
    if contains(pointer as usize)
        && let Ok(layout) = Layout::from_size_align(size, align.max(align_of::<usize>()))
        && unsafe { grow_tail_in_place(ARENA_BASE as *mut ArenaHeader, pointer, layout) }
    {
        return pointer;
    }
    let replacement = unsafe { memalign(align, size) };
    if replacement.is_null() {
        return replacement;
    }
    unsafe {
        ptr::copy_nonoverlapping(pointer, replacement, capacity);
        free(pointer);
    }
    replacement
}

#[cfg(windows)]
pub type Commit = unsafe extern "system" fn(usize, usize) -> i32;
#[cfg(windows)]
static COMMIT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Install the process memory coordinator before guest allocations begin.
#[cfg(windows)]
pub fn install_backend(entry: Commit) {
    let entry = entry as *const () as usize;
    assert!(
        COMMIT
            .compare_exchange(0, entry, Ordering::Release, Ordering::Relaxed)
            .is_ok()
    );
}
#[cfg(windows)]
fn backend() -> Option<Commit> {
    let entry = COMMIT.load(Ordering::Acquire);
    (entry != 0).then(|| unsafe { core::mem::transmute::<usize, Commit>(entry) })
}

#[cfg(windows)]
fn mapped_header() -> Option<*mut ArenaHeader> {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, VirtualQuery};
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
    if unsafe { VirtualQuery(ARENA_BASE as _, &mut info, size_of_val(&info)) } == 0
        || info.State != MEM_COMMIT
    {
        return None;
    }
    Some(ARENA_BASE as *mut ArenaHeader)
}

pub fn initialize() -> Result<(), InitError> {
    #[cfg(not(windows))]
    return super::initialize();
    #[cfg(windows)]
    loop {
        match STATE.load(Ordering::Acquire) {
            ARENA_READY => return Ok(()),
            ARENA_INITIALIZING => {
                core::hint::spin_loop();
                continue;
            }
            _ => {}
        }
        if STATE
            .compare_exchange(
                ARENA_UNCHECKED,
                ARENA_INITIALIZING,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_err()
        {
            continue;
        }
        let result = initialize_mapping();
        STATE.store(
            if result.is_ok() {
                ARENA_READY
            } else {
                ARENA_UNCHECKED
            },
            Ordering::Release,
        );
        return result;
    }
}

#[cfg(windows)]
fn initialize_mapping() -> Result<(), InitError> {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc,
        VirtualFree,
    };
    super::initialize()?;
    // Shared across provider copies, before taking any guest allocator lock.
    let _transaction = begin_shared_mappings_transaction().ok_or(InitError::AddressUnavailable)?;
    if let Some(header) = mapped_header() {
        return if unsafe { (*header).magic } == MAGIC {
            Ok(())
        } else {
            Err(InitError::IncompatibleLayout)
        };
    }
    let mode = if let Some(commit) = backend() {
        if unsafe { commit(ARENA_BASE, INITIAL_COMMIT) } == 0 {
            return Err(InitError::AddressUnavailable);
        }
        1
    } else {
        // Standalone allocator/native tests have no loader export. They retain
        // a separate lazy arena, registered as conventional private guest mm.
        let base = unsafe {
            VirtualAlloc(
                ARENA_BASE as _,
                super::ARENA_SIZE,
                MEM_RESERVE,
                PAGE_NOACCESS,
            )
        };
        if base as usize != ARENA_BASE {
            return Err(InitError::AddressUnavailable);
        }
        if unsafe { VirtualAlloc(base, INITIAL_COMMIT, MEM_COMMIT, PAGE_READWRITE) } != base {
            unsafe {
                VirtualFree(base, 0, MEM_RELEASE);
            }
            return Err(InitError::AddressUnavailable);
        }
        if !register_shared_mapping(SharedForkMapping {
            base: ARENA_BASE,
            len: super::ARENA_SIZE,
            behavior: 1,
            storage: 0,
            domain: 1,
            ..Default::default()
        }) {
            unsafe {
                VirtualFree(base, 0, MEM_RELEASE);
            }
            return Err(InitError::AddressUnavailable);
        }
        0
    };
    let header = ARENA_BASE as *mut ArenaHeader;
    unsafe {
        header.write(ArenaHeader {
            magic: MAGIC,
            lock: AtomicU32::new(0),
            _padding: mode,
            bump: HEADER_BYTES,
            committed: INITIAL_COMMIT,
            free_locks: [const { AtomicU32::new(0) }; FREE_LIST_BIN_COUNT],
            free_heads: [0; FREE_LIST_BIN_COUNT],
            local_caches: [const { LocalCache::new() }; LOCAL_CACHE_SHARDS],
            handoff_offset: 0,
            handoff_len: 0,
        });
    }
    Ok(())
}

pub(super) fn grow(end: usize) -> bool {
    #[cfg(not(windows))]
    {
        let _ = end;
        false
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, PAGE_READWRITE, VirtualAlloc};
        let Some(_transaction) = begin_shared_mappings_transaction() else {
            return false;
        };
        let header = ARENA_BASE as *mut ArenaHeader;
        let _guard = lock_arena_at(header);
        let current = unsafe { (*header).committed };
        if end <= current {
            return true;
        }
        // Grow geometrically up to 16 MiB so a 16 GiB heap cannot exhaust the
        // shared VMA table with 16,384 one-MiB views. Unused pages stay lazy.
        let minimum = current
            .saturating_add(current.min(MAX_COMMIT_BYTES))
            .min(super::ARENA_SIZE);
        let Some(wanted) =
            align_up(end.max(minimum), INITIAL_COMMIT).filter(|n| *n <= super::ARENA_SIZE)
        else {
            return false;
        };
        while unsafe { (*header).committed } < wanted {
            let current = unsafe { (*header).committed };
            let bytes = (wanted - current).min(MAX_COMMIT_BYTES);
            let address = ARENA_BASE + current;
            let success = if unsafe { (*header)._padding } == 1 {
                backend().is_some_and(|commit| unsafe { commit(address, bytes) } != 0)
            } else {
                (unsafe { VirtualAlloc(address as _, bytes, MEM_COMMIT, PAGE_READWRITE) }) as usize
                    == address
            };
            if !success {
                return false;
            }
            unsafe {
                (*header).committed = current + bytes;
            }
        }
        true
    }
}

/// Fork holds topology first, then guest allocator, then host allocator locks.
/// Do not initialize a guest heap just because a process calls fork.
#[cfg(windows)]
pub fn freeze_if_initialized() -> Option<ArenaFreezeGuard> {
    let header = mapped_header()?;
    if unsafe { (*header).magic } != MAGIC {
        return None;
    }
    Some(ArenaFreezeGuard {
        _guard: lock_arena_at(header),
        _caches: lock_all_caches_at(header),
        _bins: lock_all_bins_at(header),
    })
}

/// Repairs an operation-local copy of the frozen guest allocator metadata.
/// # Safety
/// `copy` must hold HEADER_BYTES bytes copied from the frozen guest header.
pub unsafe fn clear_copied_locks(copy: *mut u8) {
    let header = copy.cast::<ArenaHeader>();
    unsafe {
        (*header).lock.store(0, Ordering::Relaxed);
        for lock in &(*header).free_locks {
            lock.store(0, Ordering::Relaxed);
        }
        for cache in &(*header).local_caches {
            cache.lock.store(0, Ordering::Relaxed);
        }
    }
}

#[cfg(all(test, windows))]
mod tests;
