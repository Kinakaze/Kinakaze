//! Fixed-address allocator whose complete state lives inside the managed arena.
//!
//! Keeping both allocator metadata and allocation contents in one mapping makes
//! the arena self-contained: the Windows fork backend can reproduce it with a
//! single remote mapping plus `WriteProcessMemory`.

use core::alloc::{GlobalAlloc, Layout};

use core::cmp;
use core::mem::{align_of, size_of};
use core::ptr;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub mod c;
mod fork_free_ranges;
pub mod guest;
pub use fork_free_ranges::{ForkFreeError, ForkFreeStats};

pub const ARENA_BASE: usize = 0x0000_4000_0000_0000;
/// Maximum address space reserved for allocations shared across the runtime.
///
/// The Windows backend commits this reservation lazily, so raising the ceiling
/// does not charge the full reservation up front. Long-lived native workloads
/// can legitimately retain several GiB across allocators, compilers, graphics,
/// and application state. Reserving sixteen GiB costs no physical memory on
/// Windows; [`ensure_committed`] commits only pages allocations actually reach.
pub const ARENA_SIZE: usize = 16 * 1024 * 1024 * 1024;
pub const HEADER_PAGE_SIZE: usize =
    (MAPPINGS_OFFSET + size_of::<SharedMappingTable>()).next_power_of_two();
// The mapping record's former padding now carries memory ownership. Reject
// providers built against the older arena contract instead of interpreting
// their uninitialized padding as a CLONE_VM sharing decision.
const ARENA_MAGIC: u64 = 0x4352_5953_4f41_4357;
// Keep the independently shared mapping registry in the metadata page, but
// leave ample room for allocator metadata to evolve without overlapping it.
pub const MAPPINGS_OFFSET: usize = 256 * 1024;
pub const MAX_MAPPINGS: usize = 16 * 1024;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SharedForkMapping {
    pub base: usize,
    pub len: usize,
    pub behavior: u32,
    /// How the Windows child must recreate this mapping.
    ///
    /// 0 is an ordinary reservation, 1 is a pure placeholder, and 2 is a
    /// placeholder replaced by a private section view. 3 retains the original
    /// shared section through a registered native handle slot.
    pub storage: u32,
    /// Address of the section handle slot in the copied child arena.
    ///
    /// All fragments backed by one section carry the same slot, which is also
    /// the stable grouping key used while cloning the mapping.
    pub backing_slot: usize,
    /// Byte offset of this fragment in its backing section.
    pub backing_offset: u64,
    /// Legal initial view protection for retained sections; zero otherwise.
    pub view_protection: u32,
    /// 0: process-private loader state/code; 1: Linux guest address-space memory.
    /// This classification does not change ordinary fork copy semantics.
    pub domain: u32,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<SharedForkMapping>() == 48);

#[repr(C)]
struct SharedMappingTable {
    /// Windows thread id that owns the mapping transaction, or zero when free.
    ///
    /// This lock is process-shared through the fixed arena.  A thread id rather
    /// than a boolean also makes the lock re-entrant across provider DLLs: a
    /// mapping operation can hold the transaction around the host VM change and
    /// then call the ordinary registration API without opening a fork race.
    lock: AtomicU32,
    depth: u32,
    /// Creation time of the thread named by `lock`.
    ///
    /// Windows thread IDs are reused.  Keeping the kernel creation token lets
    /// a contender distinguish a live owner from an unrelated newer thread
    /// carrying the same numeric ID before recovering an abandoned lock.
    owner_creation_time: AtomicU64,
    count: u32,
    mappings: [SharedForkMapping; MAX_MAPPINGS],
}

const _: () = assert!(
    MAPPINGS_OFFSET + size_of::<SharedMappingTable>() <= HEADER_PAGE_SIZE,
    "fork mapping table must fit in the committed arena metadata area"
);

// Native allocation traffic is dominated by small and medium objects, but 256
// exact 16-byte classes through 4 KiB make every per-thread front end needlessly
// large.  Keep exact classes through 128 bytes, then use eight evenly spaced
// classes per power-of-two range.  The worst internal fragmentation is 12.5%,
// while 4 KiB..16 GiB no longer jumps directly to the next power of two.
const SIZE_CLASS_QUANTUM: usize = 16;
const LINEAR_CLASS_LIMIT: usize = 128;
const LINEAR_CLASS_COUNT: usize = LINEAR_CLASS_LIMIT / SIZE_CLASS_QUANTUM;
const SUBCLASSES_PER_DOUBLING: usize = 8;
const SUBCLASS_BITS: usize = 3;
const FIRST_GEOMETRIC_SHIFT: usize = 7; // 2^7 == 128
const GEOMETRIC_GROUP_COUNT: usize = ARENA_SIZE.trailing_zeros() as usize - FIRST_GEOMETRIC_SHIFT;
const FREE_LIST_BIN_COUNT: usize =
    LINEAR_CLASS_COUNT + GEOMETRIC_GROUP_COUNT * SUBCLASSES_PER_DOUBLING;

// Bounded shard-local front ends keep ordinary allocation away from central
// locks.  A total byte budget, instead of a separate large allowance for every
// class, lets a hot class keep a useful batch without allowing hundreds of cold
// classes to retain hundreds of MiB.  Metadata remains in the arena so fork
// snapshots preserve and freeze it.
const LOCAL_CACHE_SHARDS: usize = 256;
const LOCAL_CACHE_BIN_COUNT: usize =
    LINEAR_CLASS_COUNT + (16 - FIRST_GEOMETRIC_SHIFT) * SUBCLASSES_PER_DOUBLING;
const LOCAL_CACHE_BIN_LIMIT: u8 = 64;
const LOCAL_CACHE_TRANSFER_BATCH: usize = 32;
const LOCAL_CACHE_BYTE_LIMIT: usize = 1024 * 1024;
const LOCAL_CACHE_COLD_REFILL_BYTES: usize = 8 * 1024;
const LOCAL_CACHE_RETIRE_MIN_BYTES: usize = 64 * 1024;
#[cfg(windows)]
const RECOVERING_MAPPING_OWNER: u32 = u32::MAX;

#[repr(C)]
struct LocalCache {
    lock: AtomicU32,
    cached_bytes: usize,
    counts: [u8; LOCAL_CACHE_BIN_COUNT],
    heads: [usize; LOCAL_CACHE_BIN_COUNT],
}

impl LocalCache {
    const fn new() -> Self {
        Self {
            lock: AtomicU32::new(0),
            cached_bytes: 0,
            counts: [0; LOCAL_CACHE_BIN_COUNT],
            heads: [0; LOCAL_CACHE_BIN_COUNT],
        }
    }
}

/// Serializes one host virtual-memory mutation with the fork VMA snapshot.
///
/// On Windows the transaction is re-entrant for the owning OS thread.  This is
/// necessary because the runtime can exist in several provider DLLs while the
/// ownership word itself lives once in the shared allocator arena.
pub struct SharedMappingsTransaction(*mut SharedMappingTable);

impl Drop for SharedMappingsTransaction {
    fn drop(&mut self) {
        // SAFETY: the fixed arena remains mapped for the life of the process and
        // only the owning thread reads or writes `depth` while `lock` is nonzero.
        unsafe {
            #[cfg(windows)]
            if (*self.0).depth > 1 {
                (*self.0).depth -= 1;
                return;
            }
            (*self.0).depth = 0;
            (*self.0).owner_creation_time.store(0, Ordering::Relaxed);
            (*self.0).lock.store(0, Ordering::Release);
        }
    }
}

#[cfg(windows)]
fn thread_creation_time(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<u64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetThreadTimes;

    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetThreadTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        None
    } else {
        Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }
}

#[cfg(windows)]
fn current_thread_identity() -> Option<(u32, u64)> {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, GetCurrentThreadId};

    let id = unsafe { GetCurrentThreadId() };
    thread_creation_time(unsafe { GetCurrentThread() }).map(|created| (id, created))
}

#[cfg(windows)]
fn mapping_owner_is_abandoned(owner: u32, expected_creation_time: u64) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, GetProcessIdOfThread, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
        THREAD_SYNCHRONIZE, WaitForSingleObject,
    };

    let handle = unsafe {
        OpenThread(
            THREAD_QUERY_LIMITED_INFORMATION | THREAD_SYNCHRONIZE,
            0,
            owner,
        )
    };
    if handle.is_null() {
        return unsafe { GetLastError() } == ERROR_INVALID_PARAMETER;
    }

    let process = unsafe { GetProcessIdOfThread(handle) };
    let signalled = unsafe { WaitForSingleObject(handle, 0) } == WAIT_OBJECT_0;
    let actual_creation_time = thread_creation_time(handle);
    unsafe { CloseHandle(handle) };

    // Thread IDs are system-wide.  A live thread in another process proves the
    // original owner is gone and its ID has already been recycled.
    if process != 0 && process != unsafe { GetCurrentProcessId() } {
        return true;
    }
    if signalled {
        return true;
    }
    // Zero means the owner has acquired the word but has not published its
    // token yet.  That short publication window is never treated as death.
    expected_creation_time != 0
        && actual_creation_time.is_some_and(|actual| actual != expected_creation_time)
}

/// Begins a transaction that excludes both fork snapshots and other mapping
/// mutations until the returned guard is dropped.
pub fn begin_shared_mappings_transaction() -> Option<SharedMappingsTransaction> {
    if initialize().is_err() {
        return None;
    }
    let table = (ARENA_BASE + MAPPINGS_OFFSET) as *mut SharedMappingTable;
    #[cfg(windows)]
    let (owner, owner_creation_time) = current_thread_identity()?;
    #[cfg(not(windows))]
    let owner = 1u32;
    let mut spins = 0usize;
    // SAFETY: initialization committed the header page containing this atomic.
    loop {
        #[cfg(windows)]
        if unsafe { (*table).lock.load(Ordering::Relaxed) } == owner
            && unsafe { (*table).owner_creation_time.load(Ordering::Acquire) }
                == owner_creation_time
        {
            // SAFETY: `owner` is the current OS thread, so no other thread can
            // touch the recursion depth until the outermost guard releases it.
            unsafe { (*table).depth += 1 };
            return Some(SharedMappingsTransaction(table));
        }
        if unsafe {
            (*table)
                .lock
                .compare_exchange_weak(0, owner, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        } {
            // SAFETY: the successful acquire made this thread the sole owner.
            unsafe {
                (*table)
                    .owner_creation_time
                    .store(owner_creation_time, Ordering::Release);
                (*table).depth = 1;
            }
            return Some(SharedMappingsTransaction(table));
        }
        spins = spins.saturating_add(1);
        #[cfg(windows)]
        if spins > 100 {
            let held = unsafe { (*table).lock.load(Ordering::Acquire) };
            let held_creation_time =
                unsafe { (*table).owner_creation_time.load(Ordering::Acquire) };
            if held != 0
                && held != RECOVERING_MAPPING_OWNER
                && mapping_owner_is_abandoned(held, held_creation_time)
            {
                // Claim recovery before touching the side metadata. Multiple
                // contenders may prove the same death concurrently; exactly
                // one may move the word to the reserved recovery state.
                let claimed = unsafe {
                    (*table).lock.compare_exchange(
                        held,
                        RECOVERING_MAPPING_OWNER,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                }
                .is_ok();
                if claimed {
                    unsafe {
                        (*table).depth = 0;
                        (*table).owner_creation_time.store(0, Ordering::Relaxed);
                        (*table).lock.store(0, Ordering::Release);
                    }
                }
                spins = 0;
                continue;
            }
        }
        if spins > 100 {
            std::thread::yield_now();
        } else {
            core::hint::spin_loop();
        }
    }
}

pub fn register_shared_mapping(mapping: SharedForkMapping) -> bool {
    let SharedForkMapping {
        base,
        len,
        behavior: _,
        storage: _,
        backing_slot: _,
        backing_offset: _,
        view_protection: _,
        domain,
    } = mapping;
    if base == 0 || len == 0 || base.checked_add(len).is_none() || domain > 1 {
        return false;
    }
    let Some(transaction) = begin_shared_mappings_transaction() else {
        return false;
    };
    // SAFETY: `transaction` owns the process-wide mapping lock.
    let table = unsafe { &mut *transaction.0 };
    let mut count = (table.count as usize).min(MAX_MAPPINGS);
    let inserted = replace_mapping_range(&mut table.mappings, &mut count, mapping);
    if inserted {
        table.count = count as u32;
    }
    inserted
}

/// Replaces an address interval in the fork mapping registry.
///
/// Linux permits a page-granular mapping to replace part of an earlier reserve.
/// Keeping both overlapping records makes the fork child first materialise one
/// and later overwrite it as a placeholder, so the table itself must remain an
/// exact, non-overlapping partition of the live VMAs. The update is preflighted
/// before mutation so exhausting the fixed table never publishes half a split.
fn replace_mapping_range(
    mappings: &mut [SharedForkMapping],
    count: &mut usize,
    mapping: SharedForkMapping,
) -> bool {
    let Some(mapping_end) = mapping.base.checked_add(mapping.len) else {
        return false;
    };
    if mapping.base == 0 || mapping.len == 0 || *count > mappings.len() {
        return false;
    }

    let mut required = 1usize;
    for existing in &mappings[..*count] {
        let Some(existing_end) = existing.base.checked_add(existing.len) else {
            return false;
        };
        if existing_end <= mapping.base || mapping_end <= existing.base {
            required += 1;
        } else {
            required += usize::from(existing.base < mapping.base);
            required += usize::from(mapping_end < existing_end);
            if matches!(existing.storage, 2 | 3)
                && mapping_end < existing_end
                && existing
                    .backing_offset
                    .checked_add((mapping_end - existing.base) as u64)
                    .is_none()
            {
                return false;
            }
        }
        if required > mappings.len() {
            return false;
        }
    }

    let mut live = *count;
    let mut index = 0usize;
    while index < live {
        let existing = mappings[index];
        let existing_end = existing.base + existing.len;
        if existing_end <= mapping.base || mapping_end <= existing.base {
            index += 1;
            continue;
        }

        let keep_left = existing.base < mapping.base;
        let keep_right = mapping_end < existing_end;
        match (keep_left, keep_right) {
            (true, true) => {
                mappings[index].len = mapping.base - existing.base;
                let mut right = existing;
                right.base = mapping_end;
                right.len = existing_end - mapping_end;
                if matches!(right.storage, 2 | 3) {
                    right.backing_offset += (mapping_end - existing.base) as u64;
                }
                mappings[live] = right;
                live += 1;
                index += 1;
            }
            (true, false) => {
                mappings[index].len = mapping.base - existing.base;
                index += 1;
            }
            (false, true) => {
                mappings[index].base = mapping_end;
                mappings[index].len = existing_end - mapping_end;
                if matches!(mappings[index].storage, 2 | 3) {
                    mappings[index].backing_offset += (mapping_end - existing.base) as u64;
                }
                index += 1;
            }
            (false, false) => {
                live -= 1;
                mappings[index] = mappings[live];
                mappings[live] = SharedForkMapping::default();
            }
        }
    }

    mappings[live] = mapping;
    live += 1;
    *count = live;
    true
}

pub fn unregister_shared_mapping(base: usize) {
    if base == 0 {
        return;
    }
    let Some(transaction) = begin_shared_mappings_transaction() else {
        return;
    };
    // SAFETY: `transaction` owns the process-wide mapping lock.
    let table = unsafe { &mut *transaction.0 };
    let count = (table.count as usize).min(MAX_MAPPINGS);
    for i in 0..count {
        if table.mappings[i].base == base {
            if count > 1 {
                table.mappings[i] = table.mappings[count - 1];
                table.mappings[count - 1] = SharedForkMapping::default();
            } else {
                table.mappings[0] = SharedForkMapping::default();
            }
            table.count -= 1;
            break;
        }
    }
}

/// Look up one restored owner without allocating a copy of the VMA table.
pub fn shared_mapping(base: usize) -> Option<SharedForkMapping> {
    let transaction = begin_shared_mappings_transaction()?;
    // SAFETY: the transaction owns the shared mapping lock.
    let table = unsafe { &*transaction.0 };
    table.mappings[..(table.count as usize).min(MAX_MAPPINGS)]
        .iter()
        .find(|mapping| mapping.base == base && mapping.len != 0)
        .copied()
}

pub fn collect_shared_mappings() -> Vec<SharedForkMapping> {
    let Some(transaction) = begin_shared_mappings_transaction() else {
        return Vec::new();
    };
    // SAFETY: `transaction` owns the process-wide mapping lock.
    let table = unsafe { &*transaction.0 };
    let count = (table.count as usize).min(MAX_MAPPINGS);
    let mut res = Vec::with_capacity(count);
    for i in 0..count {
        if table.mappings[i].base != 0 && table.mappings[i].len != 0 {
            res.push(table.mappings[i]);
        }
    }
    res
}

#[repr(C)]
struct ArenaHeader {
    magic: u64,
    lock: AtomicU32,
    _padding: u32,
    bump: usize,
    /// Bytes committed from the start of the fixed reservation.
    ///
    /// Windows reserves the full arena up front but commits it lazily so a
    /// mostly empty allocator does not consume the whole reservation's commit
    /// charge.
    committed: usize,
    free_locks: [AtomicU32; FREE_LIST_BIN_COUNT],
    free_heads: [usize; FREE_LIST_BIN_COUNT],
    local_caches: [LocalCache; LOCAL_CACHE_SHARDS],
    /// Offset of the fork handoff payload, or 0 when none is staged.
    ///
    /// The arena is reproduced at the same address in the child, so state left
    /// here survives the clone. That is how the descriptor table crosses a
    /// `fork`: DLL globals are not copied, but the arena is.
    handoff_offset: usize,
    /// Length in bytes of the handoff payload.
    handoff_len: usize,
}

const _: () = assert!(
    size_of::<ArenaHeader>() <= MAPPINGS_OFFSET,
    "allocator header and shared mapping table must not overlap"
);

#[repr(C)]
struct BlockHeader {
    magic: usize,
    allocation_start: usize,
    capacity: usize,
    next_free: usize,
}

/// Stages bytes for the next `fork` to carry into the child.
///
/// Overwrites any previously staged payload.
///
/// The payload is an ordinary arena allocation rather than a small byte array in
/// the header page.  A descriptor table alone can be tens of KiB, and the runtime
/// now multiplexes every registered fork participant into the same frame.  Keeping
/// an artificial header-page limit here used to make a large fork silently lose
/// its descriptor state.
pub fn stage_handoff(payload: &[u8]) -> bool {
    if initialize().is_err() || payload.is_empty() {
        return false;
    }
    let layout = match Layout::from_size_align(payload.len(), align_of::<usize>()) {
        Ok(layout) => layout,
        Err(_) => return false,
    };
    // Allocate before retiring the old frame.  If the arena is exhausted the
    // previous frame remains intact and the caller gets an honest failure.
    // SAFETY: `layout` is valid and allocation is serialized by the arena lock.
    let destination = unsafe { allocate(layout) };
    if destination.is_null() {
        return false;
    }
    unsafe {
        ptr::copy_nonoverlapping(payload.as_ptr(), destination, payload.len());
        let header = header();
        let previous = (*header).handoff_offset;
        (*header).handoff_offset = destination as usize - ARENA_BASE;
        (*header).handoff_len = payload.len();
        if previous != 0 {
            deallocate((ARENA_BASE + previous) as *mut u8);
        }
    }
    true
}

/// Runs `consume` against the staged handoff payload without copying it.
///
/// Fork restoration calls this before DLL participants have adopted their
/// parent state.  Allocating a temporary `Vec` here would let the fresh child
/// touch the copied allocator before those owners are coherent.  The staged
/// block therefore remains allocated and immutable for the whole callback and
/// is retired only after the callback returns.
pub fn consume_handoff_with<R>(consume: impl FnOnce(&[u8]) -> R) -> Option<R> {
    if initialize().is_err() {
        return None;
    }
    // SAFETY: the arena is initialized, so the header is readable.
    let (offset, length) = unsafe {
        let header = header();
        ((*header).handoff_offset, (*header).handoff_len)
    };
    if offset < HEADER_PAGE_SIZE
        || length == 0
        || offset
            .checked_add(length)
            .is_none_or(|end| end > ARENA_SIZE)
    {
        return None;
    }
    // SAFETY: the recorded range is an allocated arena block. `stage_handoff`
    // is the only writer and cannot run concurrently with this fork-child
    // restore boundary.
    let payload =
        unsafe { core::slice::from_raw_parts((ARENA_BASE + offset) as *const u8, length) };
    let result = consume(payload);

    // Consume the frame only after its last borrower has returned so a later
    // fork cannot replay stale state. SAFETY: the header and allocation belong
    // to this process's fixed arena.
    unsafe {
        let header = header();
        (*header).handoff_offset = 0;
        (*header).handoff_len = 0;
        deallocate((ARENA_BASE + offset) as *mut u8);
    }
    Some(result)
}

/// Returns an owned copy of the staged handoff payload, if any.
///
/// General callers retain the convenient owned API. Fork restoration uses
/// [`consume_handoff_with`] so its pre-participant critical section performs no
/// allocation.
pub fn take_handoff() -> Option<Vec<u8>> {
    consume_handoff_with(<[u8]>::to_vec)
}

/// Metadata needed by the process-cloning backend.
#[derive(Clone, Copy, Debug)]
pub struct ArenaSnapshot {
    pub base: *const u8,
    pub mapped_len: usize,
    pub used_len: usize,
    pub committed_len: usize,
}

/// Exclusive allocator boundary held while a fork snapshot is copied.
///
/// Other threads may keep running Windows code, but none can publish or recycle
/// an arena allocation while this guard is live.  The Windows backend additionally
/// suspends the other threads for the actual memory copy; taking this guard first
/// avoids suspending a thread while it owns the allocator spin lock.
pub struct ArenaFreezeGuard {
    _guard: ArenaGuard,
    _caches: AllCachesGuard,
    _bins: AllBinsGuard,
}

/// Freezes allocator metadata and returns the range that is stable until the
/// guard is dropped.
pub fn freeze_snapshot() -> Result<(ArenaFreezeGuard, ArenaSnapshot), InitError> {
    initialize()?;
    let guard = lock_arena();
    let caches = lock_all_caches();
    let bins = lock_all_bins();
    // SAFETY: `guard` owns the allocator lock.
    let used_len = unsafe { (*header()).bump };
    let committed_len = unsafe { (*header()).committed };
    Ok((
        ArenaFreezeGuard {
            _guard: guard,
            _caches: caches,
            _bins: bins,
        },
        ArenaSnapshot {
            base: ARENA_BASE as *const u8,
            mapped_len: ARENA_SIZE,
            used_len,
            committed_len,
        },
    ))
}

/// Offset of the allocator lock in the cloned header.
///
/// A fork snapshot is taken while that lock is held, so the child backend must
/// reset this word after writing the arena.  Exposing the offset rather than the
/// private header layout keeps that repair explicit and checked in one crate.
pub const fn fork_lock_offset() -> usize {
    core::mem::offset_of!(ArenaHeader, lock)
}

pub const fn fork_mappings_lock_offset() -> usize {
    MAPPINGS_OFFSET + core::mem::offset_of!(SharedMappingTable, lock)
}

/// Offset and length of the allocator's size-class locks. A fork snapshot is
/// copied while all of them are held, so the child resets the copied words just
/// like the global bump lock.
pub const fn fork_bin_locks_offset() -> usize {
    core::mem::offset_of!(ArenaHeader, free_locks)
}

pub const fn fork_bin_locks_len() -> usize {
    size_of::<[AtomicU32; FREE_LIST_BIN_COUNT]>()
}

pub const fn fork_cache_lock_offset(index: usize) -> usize {
    core::mem::offset_of!(ArenaHeader, local_caches)
        + index * size_of::<LocalCache>()
        + core::mem::offset_of!(LocalCache, lock)
}

pub const fn fork_cache_count() -> usize {
    LOCAL_CACHE_SHARDS
}

// The snapshot only describes an address range. The caller is responsible for
// stopping concurrent mutation while it is copied.
unsafe impl Send for ArenaSnapshot {}
unsafe impl Sync for ArenaSnapshot {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitError {
    AddressUnavailable,
    IncompatibleLayout,
    UnsupportedPlatform,
}

/// Allocator used by Rust binaries hosted on the kinakaze runtime.
pub struct ManagedAllocator;

// Process-local initialization state. The arena itself is shared by every copy
// of this crate at one fixed address, but each PE module has its own statics and
// must validate that mapping once. Re-running VirtualQuery for every allocation
// turns allocation-heavy one-time setup (notably decoder table construction) into
// thousands of kernel transitions.
const ARENA_UNCHECKED: u32 = 0;
const ARENA_INITIALIZING: u32 = 1;
const ARENA_READY: u32 = 2;
static ARENA_STATE: AtomicU32 = AtomicU32::new(ARENA_UNCHECKED);
static NEXT_LOCAL_CACHE: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static LOCAL_CACHE_INDEX: std::cell::Cell<usize> = const {
        std::cell::Cell::new(usize::MAX)
    };
    static LAST_COLD_CLASS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(usize::MAX)
    };
}

fn local_cache_index() -> usize {
    LOCAL_CACHE_INDEX.with(|index| {
        let current = index.get();
        if current != usize::MAX {
            return current;
        }
        let assigned =
            NEXT_LOCAL_CACHE.fetch_add(1, Ordering::Relaxed) as usize % LOCAL_CACHE_SHARDS;
        index.set(assigned);
        assigned
    })
}

unsafe impl GlobalAlloc for ManagedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `GlobalAlloc` makes allocation an unsafe operation; this type
        // serializes all changes to its arena metadata.
        unsafe { allocate(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        // SAFETY: the caller promises that `ptr` was returned by this allocator.
        unsafe { deallocate(ptr) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: same contract as `GlobalAlloc::realloc`.
        unsafe { reallocate(ptr, layout.align(), new_size) }
    }
}

/// Ensures the arena is reserved at the address shared by parent and children.
pub fn initialize() -> Result<(), InitError> {
    loop {
        match ARENA_STATE.load(Ordering::Acquire) {
            ARENA_READY => return Ok(()),
            ARENA_INITIALIZING => {
                core::hint::spin_loop();
                continue;
            }
            ARENA_UNCHECKED => {}
            _ => unreachable!(),
        }
        if ARENA_STATE
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

        let result = initialize_arena_mapping();
        ARENA_STATE.store(
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

fn initialize_arena_mapping() -> Result<(), InitError> {
    let current = header();
    // SAFETY: reading the magic is valid only when the fixed address is mapped.
    // `mapping_exists` performs that platform-specific check first.
    if mapping_exists() {
        return if unsafe { ptr::read_volatile(ptr::addr_of!((*current).magic)) } == ARENA_MAGIC {
            Ok(())
        } else {
            // Another provider already owns this address. Never attempt a new
            // allocation/header initialization over an incompatible live arena.
            Err(InitError::IncompatibleLayout)
        };
    }

    let mapped = map_arena()?;
    if mapped as usize != ARENA_BASE {
        return Err(InitError::AddressUnavailable);
    }

    // SAFETY: a successful `map_arena` returns a writable ARENA_SIZE mapping.
    unsafe {
        ptr::write(
            current,
            ArenaHeader {
                magic: ARENA_MAGIC,
                lock: AtomicU32::new(0),
                _padding: 0,
                bump: HEADER_PAGE_SIZE,
                committed: if cfg!(windows) {
                    HEADER_PAGE_SIZE
                } else {
                    ARENA_SIZE
                },
                free_locks: [const { AtomicU32::new(0) }; FREE_LIST_BIN_COUNT],
                free_heads: [0; FREE_LIST_BIN_COUNT],
                local_caches: [const { LocalCache::new() }; LOCAL_CACHE_SHARDS],
                handoff_offset: 0,
                handoff_len: 0,
            },
        );
        ptr::write_bytes(
            (ARENA_BASE + MAPPINGS_OFFSET) as *mut u8,
            0,
            size_of::<SharedMappingTable>(),
        );
    }
    Ok(())
}

/// Returns the arena range that must be copied to reproduce allocator state.
pub fn snapshot() -> Result<ArenaSnapshot, InitError> {
    initialize()?;
    let guard = lock_arena();
    // SAFETY: initialization established the header mapping and the lock keeps
    // `bump` stable while it is sampled.
    let used_len = unsafe { (*header()).bump };
    let committed_len = unsafe { (*header()).committed };
    drop(guard);
    Ok(ArenaSnapshot {
        base: ARENA_BASE as *const u8,
        mapped_len: ARENA_SIZE,
        used_len,
        committed_len,
    })
}

/// Allocates a C-compatible block with at least pointer alignment.
///
/// # Safety
///
/// The returned allocation must be released only through this allocator. The
/// caller must handle a null result without dereferencing it.
pub unsafe fn malloc(size: usize) -> *mut u8 {
    let size = size.max(1);
    let layout = match Layout::from_size_align(size, align_of::<usize>() * 2) {
        Ok(layout) => layout,
        Err(_) => return ptr::null_mut(),
    };
    // SAFETY: forwarded to the allocator implementation.
    unsafe { allocate(layout) }
}

/// Allocates memory with a specified alignment.
///
/// # Safety
///
/// `align` must be a power of two. The returned block must be released with [`free`].
pub unsafe fn memalign(align: usize, size: usize) -> *mut u8 {
    let size = size.max(1);
    let align = align.max(align_of::<usize>());
    let layout = match Layout::from_size_align(size, align) {
        Ok(layout) => layout,
        Err(_) => return ptr::null_mut(),
    };
    // SAFETY: forwarded to the allocator implementation.
    unsafe { allocate(layout) }
}

/// Frees a block previously returned by [`malloc`] or [`reallocate`].
///
/// # Safety
///
/// `ptr` must be null or a live block owned by this allocator, and it must not
/// be used again after this call.
pub unsafe fn free(ptr: *mut u8) {
    if ptr.is_null() || !contains(ptr as usize) {
        return;
    }
    // SAFETY: caller upholds the ownership requirement.
    unsafe { deallocate(ptr) }
}

/// Returns the reusable capacity recorded immediately before `ptr`.
///
/// # Safety
///
/// `ptr` must be null or a live block owned by this allocator.
pub unsafe fn usable_size(ptr: *mut u8) -> usize {
    if ptr.is_null() || !contains(ptr as usize) {
        return 0;
    }
    // SAFETY: all allocator results have a `BlockHeader` immediately before them.
    unsafe { (*block_header(ptr)).capacity }
}

/// Resizes an allocation while preserving its prefix.
///
/// # Safety
///
/// `ptr` must be null or a live block owned by this allocator. `align` must be
/// compatible with the original allocation when `ptr` is non-null.
pub unsafe fn reallocate(ptr: *mut u8, align: usize, new_size: usize) -> *mut u8 {
    if ptr.is_null() {
        let layout = match Layout::from_size_align(new_size.max(1), align.max(1)) {
            Ok(layout) => layout,
            Err(_) => return ptr::null_mut(),
        };
        // SAFETY: allocating a new independent block.
        return unsafe { allocate(layout) };
    }
    if new_size == 0 {
        // SAFETY: caller owns `ptr`.
        unsafe { deallocate(ptr) };
        return ptr::null_mut();
    }

    let item = unsafe { block_header(ptr) };
    let capacity = unsafe { (*item).capacity };

    if new_size <= capacity {
        return ptr;
    }

    let layout = match Layout::from_size_align(new_size, align.max(1)) {
        Ok(layout) => layout,
        Err(_) => return ptr::null_mut(),
    };
    if unsafe { grow_tail_in_place(header(), ptr, layout) } {
        return ptr;
    }
    // SAFETY: allocate/copy/free implement the standard realloc operation.
    let replacement = unsafe { allocate(layout) };
    if replacement.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: both regions are valid and non-overlapping allocations.
    unsafe { ptr::copy_nonoverlapping(ptr, replacement, cmp::min(capacity, new_size)) };
    // SAFETY: the old allocation is no longer used after the copy.
    unsafe { deallocate(ptr) };
    replacement
}

const BLOCK_ALLOCATED: usize = 0xAAAA_AAAA_AAAA_AAAA;
const BLOCK_MAGIC: usize = 0x414C_4F43_4154_4F52; // 'ALOCATOR'
// A long-lived process can accumulate hundreds of thousands of differently
// sized free blocks. Keep a miss bounded so one allocation cannot monopolize a
// size class while walking its list.
const MAX_FREE_LIST_SCAN: usize = 256;

/// Extend the live last block without allocating or copying its prefix. Growth
/// uses the usual canonical capacity, so the resized block remains reusable in
/// the existing bins. No allocator metadata layout or fork format changes.
unsafe fn grow_tail_in_place(arena: *mut ArenaHeader, payload: *mut u8, layout: Layout) -> bool {
    // Small reallocations can use shard-local bins. Taking the central bump
    // lock for those misses adds contention to IPC's short-lived Vec buffers.
    // Reserve the tail optimization for buffers outside the local cache range.
    if layout.size() <= 64 * 1024 {
        return false;
    }
    if payload as usize & (layout.align() - 1) != 0 {
        return false;
    }
    let Some(wanted) = allocation_capacity(layout.size()) else {
        return false;
    };
    let offset = payload as usize - arena as usize;
    let Some(end) = offset.checked_add(wanted).filter(|&end| end <= ARENA_SIZE) else {
        return false;
    };
    loop {
        let guard = lock_arena_at(arena);
        let item = unsafe { block_header(payload) };
        if unsafe {
            (*item).magic != BLOCK_MAGIC
                || (*item).next_free != BLOCK_ALLOCATED
                || offset + (*item).capacity != (*arena).bump
        } {
            return false;
        }
        if end > unsafe { (*arena).committed } {
            if arena as usize == guest::ARENA_BASE {
                // Fork takes the mapping transaction before allocator locks.
                // Recheck the tail after growing: another thread may allocate.
                drop(guard);
                if !guest::grow(end) {
                    return false;
                }
                continue;
            }
            if !ensure_committed(end) {
                return false;
            }
        }
        unsafe {
            (*item).capacity = wanted;
            (*arena).bump = end;
        }
        return true;
    }
}

fn allocation_capacity(size: usize) -> Option<usize> {
    let aligned = align_up(
        size.max(1),
        SIZE_CLASS_QUANTUM.max(align_of::<BlockHeader>()),
    )?;
    if aligned > ARENA_SIZE {
        return None;
    }
    if aligned <= LINEAR_CLASS_LIMIT {
        Some(aligned)
    } else {
        let range_shift = (usize::BITS - aligned.saturating_sub(1).leading_zeros() - 1) as usize;
        let step = 1usize.checked_shl(range_shift.saturating_sub(SUBCLASS_BITS) as u32)?;
        align_up(aligned, step)
    }
}

fn free_bin_index(capacity: usize) -> usize {
    let capacity = allocation_capacity(capacity).unwrap_or(ARENA_SIZE);
    canonical_bin_index(capacity)
}

/// Return substantial thread caches to the shared bins at guest thread
/// retirement. This never initializes a heap, allocates storage, or releases
/// committed pages. Live blocks, including values passed to pthread_join, stay
/// owned by their callers. A shard may be shared after the index wraps.
pub fn release_thread_cache() {
    let Ok(shard) = LOCAL_CACHE_INDEX.try_with(|index| index.get()) else {
        return;
    };
    if shard == usize::MAX {
        return;
    }
    if ARENA_STATE.load(Ordering::Acquire) == ARENA_READY {
        unsafe { drain_local_cache(header(), shard) };
    }
    guest::release_thread_cache(shard);
    let _ = LAST_COLD_CLASS.try_with(|class| class.set(usize::MAX));
}

unsafe fn drain_local_cache(arena: *mut ArenaHeader, shard: usize) {
    // Cache -> bin matches allocation and fork's arena -> caches -> bins order.
    // Splice a whole class with one central lock, rather than freeing each block
    // separately. The cache lock also excludes users of a wrapped shard index.
    let _cache_guard = lock_cache_at(arena, shard);
    let cache = unsafe { &mut (*arena).local_caches[shard] };
    // Preserve a small working set for shard reuse. Flushing a few tiny objects
    // on every worker exit disrupts concurrent users' locality and adds central
    // contention. Retired shards retain less than 64 KiB, rather than up to 1 MiB.
    if cache.cached_bytes < LOCAL_CACHE_RETIRE_MIN_BYTES {
        return;
    }
    for bin in 0..LOCAL_CACHE_BIN_COUNT {
        let head = cache.heads[bin];
        let mut tail = 0;
        let mut item = head;
        for _ in 0..cache.counts[bin] {
            if !contains_in(arena, item) || item % align_of::<BlockHeader>() != 0 {
                break;
            }
            tail = item;
            item = unsafe { (*(item as *const BlockHeader)).next_free };
        }
        if tail != 0 {
            let _central = lock_bin_at(arena, bin);
            unsafe {
                (*(tail as *mut BlockHeader)).next_free = (*arena).free_heads[bin];
                (*arena).free_heads[bin] = head;
            }
        }
        cache.heads[bin] = 0;
        cache.counts[bin] = 0;
    }
    cache.cached_bytes = 0;
}

// Allocation already rounded the request to a canonical capacity. Do not
// repeat its checked alignment and leading-zero scan on every cache lookup.
fn canonical_bin_index(capacity: usize) -> usize {
    if capacity <= LINEAR_CLASS_LIMIT {
        return capacity.saturating_sub(1) / SIZE_CLASS_QUANTUM;
    }
    let range_shift = (usize::BITS - capacity.saturating_sub(1).leading_zeros() - 1) as usize;
    let range_base = 1usize << range_shift;
    let step = 1usize << (range_shift - SUBCLASS_BITS);
    let within_range = (capacity - range_base) / step - 1;
    let index = LINEAR_CLASS_COUNT
        + (range_shift - FIRST_GEOMETRIC_SHIFT) * SUBCLASSES_PER_DOUBLING
        + within_range;
    index.min(FREE_LIST_BIN_COUNT - 1)
}

fn local_cache_has_room(cache: &LocalCache, capacity: usize, bin: usize) -> bool {
    if cache.counts[bin] >= LOCAL_CACHE_BIN_LIMIT {
        return false;
    } else {
        cache
            .cached_bytes
            .checked_add(capacity)
            .is_some_and(|bytes| bytes <= LOCAL_CACHE_BYTE_LIMIT)
    }
}

unsafe fn take_local_block(
    arena: *mut ArenaHeader,
    wanted: usize,
    align: usize,
    bin: usize,
) -> Option<*mut u8> {
    if bin >= LOCAL_CACHE_BIN_COUNT {
        return None;
    }
    let shard = local_cache_index();
    let _guard = lock_cache_at(arena, shard);
    let cache = unsafe { &mut (*arena).local_caches[shard] };
    let original_count = cache.counts[bin];
    let mut link = &mut cache.heads[bin] as *mut usize;
    let mut item = unsafe { *link };
    for visited in 0..usize::from(original_count) {
        if item == 0 {
            cache.counts[bin] = visited as u8;
            cache.cached_bytes = cache
                .cached_bytes
                .saturating_sub(usize::from(original_count.saturating_sub(visited as u8)) * wanted);
            break;
        }
        if !contains_in(arena, item) || item % align_of::<BlockHeader>() != 0 {
            unsafe { *link = 0 };
            cache.counts[bin] = visited as u8;
            cache.cached_bytes = cache
                .cached_bytes
                .saturating_sub(usize::from(original_count.saturating_sub(visited as u8)) * wanted);
            break;
        }
        let item_header = item as *mut BlockHeader;
        let payload = unsafe { item_header.add(1) as *mut u8 };
        if unsafe { (*item_header).capacity } == wanted && (payload as usize) & (align - 1) == 0 {
            unsafe {
                *link = (*item_header).next_free;
                (*item_header).next_free = BLOCK_ALLOCATED;
            }
            cache.counts[bin] = cache.counts[bin].saturating_sub(1);
            cache.cached_bytes = cache.cached_bytes.saturating_sub(wanted);
            return Some(payload);
        }
        link = unsafe { ptr::addr_of_mut!((*item_header).next_free) };
        item = unsafe { *link };
    }
    // A miss refills a bounded batch under one central lock. Keep cache -> bin
    // order, matching fork's freeze order; never take a cache lock from a bin.
    let _central = lock_bin_at(arena, bin);
    let mut link = unsafe { ptr::addr_of_mut!((*arena).free_heads[bin]) };
    let mut item = unsafe { *link };
    let mut result = None;
    let mut cached = 0;
    for _ in 0..MAX_FREE_LIST_SCAN {
        if item == 0
            || (result.is_some()
                && (cached == LOCAL_CACHE_TRANSFER_BATCH - 1
                    || !local_cache_has_room(cache, wanted, bin)))
        {
            break;
        }
        if !contains_in(arena, item) || item % align_of::<BlockHeader>() != 0 {
            unsafe { *link = 0 };
            break;
        }
        let item_header = item as *mut BlockHeader;
        let payload = unsafe { item_header.add(1) as *mut u8 };
        if unsafe { (*item_header).capacity } == wanted && (payload as usize) & (align - 1) == 0 {
            unsafe { *link = (*item_header).next_free };
            if result.is_none() {
                unsafe { (*item_header).next_free = BLOCK_ALLOCATED };
                result = Some(payload);
            } else {
                unsafe { (*item_header).next_free = cache.heads[bin] };
                cache.heads[bin] = item;
                cache.counts[bin] += 1;
                cache.cached_bytes += wanted;
                cached += 1;
            }
        } else {
            link = unsafe { ptr::addr_of_mut!((*item_header).next_free) };
        }
        item = unsafe { *link };
    }
    result
}

unsafe fn cache_free_block(arena: *mut ArenaHeader, item: *mut BlockHeader, bin: usize) -> bool {
    let capacity = unsafe { (*item).capacity };
    if bin >= LOCAL_CACHE_BIN_COUNT {
        return false;
    }
    let shard = local_cache_index();
    let _guard = lock_cache_at(arena, shard);
    let cache = unsafe { &mut (*arena).local_caches[shard] };
    if !local_cache_has_room(cache, capacity, bin) {
        if cache.counts[bin] == 0 {
            return false;
        }
        // Spill half a full class at once. Subsequent frees stay local instead
        // of contending on the central bin for every object in a freed batch.
        let _central = lock_bin_at(arena, bin);
        let first = cache.heads[bin];
        let mut next = first;
        let mut tail = ptr::null_mut::<BlockHeader>();
        let mut moved = 0;
        for _ in 0..LOCAL_CACHE_TRANSFER_BATCH.min(usize::from(cache.counts[bin])) {
            if !contains_in(arena, next) || next % align_of::<BlockHeader>() != 0 {
                cache.cached_bytes = cache
                    .cached_bytes
                    .saturating_sub((usize::from(cache.counts[bin]) - moved) * capacity);
                cache.heads[bin] = 0;
                cache.counts[bin] = moved as u8;
                next = 0;
                break;
            }
            tail = next as *mut BlockHeader;
            next = unsafe { (*tail).next_free };
            moved += 1;
        }
        if !tail.is_null() {
            // Transfer the existing chain. Only its tail needs rewriting;
            // intermediate headers can remain in shared/read-only CPU caches.
            unsafe {
                (*tail).next_free = (*arena).free_heads[bin];
                (*arena).free_heads[bin] = first;
            }
            cache.heads[bin] = next;
            cache.counts[bin] -= moved as u8;
            cache.cached_bytes = cache.cached_bytes.saturating_sub(moved * capacity);
        }
        if !local_cache_has_room(cache, capacity, bin) {
            return false;
        }
    }
    unsafe {
        (*item).next_free = cache.heads[bin];
        cache.heads[bin] = item as usize;
    }
    cache.counts[bin] += 1;
    cache.cached_bytes += capacity;
    true
}

unsafe fn insert_free_block(arena: *mut ArenaHeader, item: *mut BlockHeader, bin: usize) {
    let _guard = lock_bin_at(arena, bin);
    unsafe {
        (*item).next_free = (*arena).free_heads[bin];
        (*arena).free_heads[bin] = item as usize;
    }
}

/// Called with the bump lock held, after satisfying the requested allocation.
/// Populate a short cold batch without committing extra pages or clearing payloads.
/// Arena -> cache matches the snapshot lock order; the hot path stays local.
unsafe fn refill_committed_blocks(
    arena: *mut ArenaHeader,
    wanted: usize,
    align: usize,
    bin: usize,
) {
    if wanted > 4096 || align > 16 {
        return;
    }
    // Repeated misses in one class benefit from a batch. Interleaved classes
    // keep bump order instead of spreading adjacent application objects over
    // many pages. This thread-local hint owns no blocks and needs no fork repair.
    let class = arena as usize + bin;
    if !LAST_COLD_CLASS.with(|previous| previous.replace(class) == class) {
        return;
    }
    let shard = local_cache_index();
    let _cache_guard = lock_cache_at(arena, shard);
    let cache = unsafe { &mut (*arena).local_caches[shard] };
    let count = (LOCAL_CACHE_TRANSFER_BATCH - 1).min(LOCAL_CACHE_COLD_REFILL_BYTES / wanted);
    for _ in 0..count {
        if !local_cache_has_room(cache, wanted, bin) {
            break;
        }
        let start = unsafe { (*arena).bump };
        // The arena is at most 16 GiB; these additions cannot overflow usize.
        let payload = (start + size_of::<BlockHeader>() + align - 1) & !(align - 1);
        let end = payload + wanted;
        if end > unsafe { (*arena).committed } {
            break;
        }
        let block = (arena as usize + payload - size_of::<BlockHeader>()) as *mut BlockHeader;
        unsafe {
            block.write(BlockHeader {
                magic: BLOCK_MAGIC,
                allocation_start: start,
                capacity: wanted,
                next_free: cache.heads[bin],
            });
            (*arena).bump = end;
        }
        cache.heads[bin] = block as usize;
        cache.counts[bin] += 1;
        cache.cached_bytes += wanted;
    }
}

unsafe fn allocate(layout: Layout) -> *mut u8 {
    if initialize().is_err() {
        return ptr::null_mut();
    }
    unsafe { allocate_from(header(), layout) }
}

unsafe fn allocate_from(arena: *mut ArenaHeader, layout: Layout) -> *mut u8 {
    let Some(wanted) = allocation_capacity(layout.size()) else {
        return ptr::null_mut();
    };
    let align = layout.align().max(align_of::<BlockHeader>());
    let bin = canonical_bin_index(wanted);

    if let Some(payload) = unsafe { take_local_block(arena, wanted, align, bin) } {
        return payload;
    }

    // Every block in this bin has the same canonical capacity. Keeping the
    // lookup within one class makes reuse deterministic and prevents splitting
    // from manufacturing unbounded, differently-sized large blocks.
    // Small classes already searched their central bin during the local refill.
    if bin >= LOCAL_CACHE_BIN_COUNT {
        let _guard = lock_bin_at(arena, bin);
        let mut link = unsafe { ptr::addr_of_mut!((*arena).free_heads[bin]) };
        // SAFETY: all non-zero links were constructed from valid block headers.
        let mut item = unsafe { *link };
        for _ in 0..MAX_FREE_LIST_SCAN {
            if item == 0 {
                break;
            }
            if !contains_in(arena, item) || item % align_of::<BlockHeader>() != 0 {
                // Corrupted or misaligned free list link; discard the invalid
                // remainder while preserving every other class.
                unsafe { *link = 0 };
                break;
            }
            let item_header = item as *mut BlockHeader;
            let payload = unsafe { item_header.add(1) as *mut u8 };
            if unsafe { (*item_header).capacity } == wanted && (payload as usize) & (align - 1) == 0
            {
                unsafe {
                    *link = (*item_header).next_free;
                    (*item_header).next_free = BLOCK_ALLOCATED;
                }
                return payload;
            }
            link = unsafe { ptr::addr_of_mut!((*item_header).next_free) };
            item = unsafe { *link };
        }
        drop(_guard);
    }

    loop {
        let guard = lock_arena_at(arena);
        // SAFETY: `bump` is protected by the arena lock.
        let allocation_start = unsafe { (*arena).bump };
        let header_end = match allocation_start.checked_add(size_of::<BlockHeader>()) {
            Some(value) => value,
            None => return ptr::null_mut(),
        };
        let payload_offset = match align_up(header_end, align) {
            Some(value) => value,
            None => return ptr::null_mut(),
        };
        let allocation_end = match payload_offset.checked_add(wanted) {
            Some(value) if value <= ARENA_SIZE => value,
            _ => return ptr::null_mut(),
        };
        if arena as usize == guest::ARENA_BASE && allocation_end > unsafe { (*arena).committed } {
            // Never wait for the mapping transaction while holding a guest heap
            // lock: fork owns the transaction before freezing the heap.
            drop(guard);
            if !guest::grow(allocation_end) {
                return ptr::null_mut();
            }
            continue;
        }
        if arena as usize == ARENA_BASE && !ensure_committed(allocation_end) {
            return ptr::null_mut();
        }
        let item_header =
            (arena as usize + payload_offset - size_of::<BlockHeader>()) as *mut BlockHeader;
        // SAFETY: the computed header and payload lie inside the writable mapping.
        unsafe {
            ptr::write(
                item_header,
                BlockHeader {
                    magic: BLOCK_MAGIC,
                    allocation_start,
                    capacity: wanted,
                    next_free: BLOCK_ALLOCATED,
                },
            );
            (*arena).bump = allocation_end;
            refill_committed_blocks(arena, wanted, align, bin);
        }
        return (arena as usize + payload_offset) as *mut u8;
    }
}

unsafe fn deallocate(ptr: *mut u8) {
    if ptr.is_null() || !contains(ptr as usize) || initialize().is_err() {
        return;
    }
    unsafe { deallocate_from(header(), ptr) };
}

unsafe fn deallocate_from(arena: *mut ArenaHeader, ptr: *mut u8) {
    if ptr.is_null()
        || !contains_in(arena, ptr as usize)
        || (ptr as usize) < arena as usize + metadata_bytes(arena) + size_of::<BlockHeader>()
    {
        return;
    }
    let item = unsafe { block_header(ptr) };
    if (item as usize) % align_of::<BlockHeader>() != 0 {
        return;
    }
    // SAFETY: the caller owns this live block until it is published under the
    // corresponding size-class lock.
    unsafe {
        if (*item).magic != BLOCK_MAGIC
            || (*item).next_free != BLOCK_ALLOCATED
            || (*item).capacity > ARENA_SIZE
        {
            // Not a valid live block header. Prevent free list corruption.
            return;
        }
    }
    // Large transient buffers can give the bump cursor back immediately.
    // Retain committed pages for the next allocation: no mapping transaction,
    // decommit fault or free-list publication is needed. Small frees keep the
    // shard-local path and never contend on the bump lock.
    if unsafe { (*item).capacity > 64 * 1024 } {
        let _guard = lock_arena_at(arena);
        if unsafe { ptr as usize - arena as usize + (*item).capacity == (*arena).bump } {
            unsafe {
                (*arena).bump = (*item).allocation_start;
                (*item).magic = 0;
                (*item).next_free = 0;
            }
            return;
        }
    }
    // Both fresh allocation and in-place growth store canonical capacities.
    let bin = canonical_bin_index(unsafe { (*item).capacity });
    if !unsafe { cache_free_block(arena, item, bin) } {
        unsafe { insert_free_block(arena, item, bin) };
    }
}

fn contains(address: usize) -> bool {
    contains_in(header(), address)
}

fn metadata_bytes(arena: *mut ArenaHeader) -> usize {
    if arena as usize == guest::ARENA_BASE {
        guest::HEADER_BYTES
    } else {
        HEADER_PAGE_SIZE
    }
}

fn contains_in(arena: *mut ArenaHeader, address: usize) -> bool {
    let base = arena as usize;
    (base + metadata_bytes(arena)..base + ARENA_SIZE).contains(&address)
}

unsafe fn block_header(ptr: *mut u8) -> *mut BlockHeader {
    // SAFETY: caller guarantees `ptr` came from this allocator.
    unsafe { ptr.sub(size_of::<BlockHeader>()) as *mut BlockHeader }
}

fn header() -> *mut ArenaHeader {
    ARENA_BASE as *mut ArenaHeader
}

fn align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
}

struct ArenaGuard(*mut ArenaHeader);

impl Drop for ArenaGuard {
    fn drop(&mut self) {
        // SAFETY: a guard is constructed only after arena initialization.
        let lock = unsafe { &(*self.0).lock };
        unlock_word(lock);
    }
}

fn lock_arena() -> ArenaGuard {
    lock_arena_at(header())
}

fn lock_arena_at(arena: *mut ArenaHeader) -> ArenaGuard {
    // SAFETY: callers initialize the chosen arena before taking its lock.
    let lock = unsafe { &(*arena).lock };
    lock_word(lock);
    ArenaGuard(arena)
}

struct BinGuard {
    arena: *mut ArenaHeader,
    index: usize,
}

impl Drop for BinGuard {
    fn drop(&mut self) {
        let lock = unsafe { &(*self.arena).free_locks[self.index] };
        unlock_word(lock);
    }
}

fn lock_bin_at(arena: *mut ArenaHeader, index: usize) -> BinGuard {
    let lock = unsafe { &(*arena).free_locks[index] };
    lock_word(lock);
    BinGuard { arena, index }
}

struct CacheGuard {
    arena: *mut ArenaHeader,
    index: usize,
}

impl Drop for CacheGuard {
    fn drop(&mut self) {
        let lock = unsafe { &(*self.arena).local_caches[self.index].lock };
        unlock_word(lock);
    }
}

fn lock_cache_at(arena: *mut ArenaHeader, index: usize) -> CacheGuard {
    let lock = unsafe { &(*arena).local_caches[index].lock };
    lock_word(lock);
    CacheGuard { arena, index }
}

struct AllCachesGuard {
    arena: *mut ArenaHeader,
    locked: usize,
}

impl Drop for AllCachesGuard {
    fn drop(&mut self) {
        for index in (0..self.locked).rev() {
            let lock = unsafe { &(*self.arena).local_caches[index].lock };
            unlock_word(lock);
        }
    }
}

fn lock_all_caches() -> AllCachesGuard {
    lock_all_caches_at(header())
}

fn lock_all_caches_at(arena: *mut ArenaHeader) -> AllCachesGuard {
    let mut guard = AllCachesGuard { arena, locked: 0 };
    while guard.locked < LOCAL_CACHE_SHARDS {
        let lock = unsafe { &(*arena).local_caches[guard.locked].lock };
        lock_word(lock);
        guard.locked += 1;
    }
    guard
}

struct AllBinsGuard {
    arena: *mut ArenaHeader,
    locked: usize,
}

impl Drop for AllBinsGuard {
    fn drop(&mut self) {
        for index in (0..self.locked).rev() {
            let lock = unsafe { &(*self.arena).free_locks[index] };
            unlock_word(lock);
        }
    }
}

fn lock_all_bins() -> AllBinsGuard {
    lock_all_bins_at(header())
}

fn lock_all_bins_at(arena: *mut ArenaHeader) -> AllBinsGuard {
    let mut guard = AllBinsGuard { arena, locked: 0 };
    while guard.locked < FREE_LIST_BIN_COUNT {
        let lock = unsafe { &(*arena).free_locks[guard.locked] };
        lock_word(lock);
        guard.locked += 1;
    }
    guard
}

fn lock_word(lock: &AtomicU32) {
    if lock
        .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        return;
    }
    // Allocation critical sections normally last only a few dozen cycles. Give
    // the current owner enough time to finish before paying for a kernel park;
    // long stalls still leave the CPU through WaitOnAddress below.
    for _ in 0..1_024 {
        // Contenders can share this cache line while the owner is busy. A
        // failed read-modify-write on every spin otherwise bounces ownership
        // between cores even though nobody can enter the critical section.
        if lock.load(Ordering::Relaxed) == 0
            && lock
                .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        {
            return;
        }
        core::hint::spin_loop();
    }
    loop {
        // State 2 records that unlock must wake a kernel waiter. Keeping the
        // marker while a woken thread owns the lock prevents stranded peers.
        if lock.swap(2, Ordering::Acquire) == 0 {
            return;
        }
        wait_for_arena_unlock(lock);
    }
}

fn unlock_word(lock: &AtomicU32) {
    let previous = lock.swap(0, Ordering::Release);
    if previous == 2 {
        wake_one_arena_waiter(lock);
    }
}

#[cfg(windows)]
fn wait_for_arena_unlock(lock: &AtomicU32) {
    use windows_sys::Win32::System::Threading::{INFINITE, WaitOnAddress};

    let contended = 2u32;
    unsafe {
        WaitOnAddress(
            (lock as *const AtomicU32).cast(),
            (&raw const contended).cast(),
            size_of::<u32>(),
            INFINITE,
        );
    }
}

#[cfg(not(windows))]
fn wait_for_arena_unlock(_lock: &AtomicU32) {
    std::thread::yield_now();
}

#[cfg(windows)]
fn wake_one_arena_waiter(lock: &AtomicU32) {
    use windows_sys::Win32::System::Threading::WakeByAddressSingle;

    unsafe { WakeByAddressSingle((lock as *const AtomicU32).cast()) };
}

#[cfg(not(windows))]
fn wake_one_arena_waiter(_lock: &AtomicU32) {}

#[cfg(windows)]
fn mapping_exists() -> bool {
    use core::mem::MaybeUninit;
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, VirtualQuery};

    let mut info = MaybeUninit::<MEMORY_BASIC_INFORMATION>::uninit();
    // SAFETY: `info` points to enough writable storage for VirtualQuery.
    let result = unsafe {
        VirtualQuery(
            ARENA_BASE as *const _,
            info.as_mut_ptr(),
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if result == 0 {
        return false;
    }
    // SAFETY: non-zero return initialized the structure.
    let info = unsafe { info.assume_init() };
    info.State == MEM_COMMIT && info.AllocationBase as usize == ARENA_BASE
}

#[cfg(windows)]
fn ensure_committed(end: usize) -> bool {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, PAGE_READWRITE, VirtualAlloc};

    // Grow in allocation-granularity chunks. The arena lock is held by every
    // caller, so the shared committed boundary can be updated non-atomically.
    let current = unsafe { (*header()).committed };
    if end <= current {
        return true;
    }
    let Some(commit_end) = align_up(end, HEADER_PAGE_SIZE) else {
        return false;
    };
    let commit_end = commit_end.min(ARENA_SIZE);
    if commit_end <= current {
        return false;
    }
    let address = ARENA_BASE + current;
    let mapped = unsafe {
        VirtualAlloc(
            address as *const _,
            commit_end - current,
            MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    if mapped as usize != address {
        return false;
    }
    unsafe { (*header()).committed = commit_end };
    true
}

#[cfg(not(windows))]
fn ensure_committed(_end: usize) -> bool {
    true
}

#[cfg(windows)]
fn map_arena() -> Result<*mut u8, InitError> {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc,
        VirtualFree,
    };

    // Reserve the stable address range without charging the process for the
    // entire gigabyte. Only the metadata page is needed before the first
    // allocation; subsequent allocations commit additional chunks.
    let reserved = unsafe {
        VirtualAlloc(
            ARENA_BASE as *const _,
            ARENA_SIZE,
            MEM_RESERVE,
            PAGE_NOACCESS,
        )
    };
    if reserved as usize != ARENA_BASE {
        return Err(InitError::AddressUnavailable);
    }
    let committed = unsafe {
        VirtualAlloc(
            ARENA_BASE as *const _,
            HEADER_PAGE_SIZE,
            MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    if committed as usize != ARENA_BASE {
        unsafe { VirtualFree(reserved, 0, MEM_RELEASE) };
        return Err(InitError::AddressUnavailable);
    }
    Ok(reserved.cast())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn mapping_exists() -> bool {
    use core::ffi::c_void;

    unsafe extern "C" {
        fn mincore(address: *mut c_void, length: usize, vector: *mut u8) -> i32;
    }

    let mut residency = 0u8;
    // SAFETY: `mincore` reports ENOMEM for an unmapped page without reading it.
    let mapped = unsafe { mincore(ARENA_BASE as *mut c_void, 4096, &mut residency) } == 0;
    mapped && unsafe { ptr::read_volatile(ARENA_BASE as *const u64) == ARENA_MAGIC }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn map_arena() -> Result<*mut u8, InitError> {
    use core::ffi::c_void;
    const PROT_READ: i32 = 1;
    const PROT_WRITE: i32 = 2;
    const MAP_PRIVATE: i32 = 2;
    const MAP_ANONYMOUS: i32 = 0x20;
    const MAP_FIXED_NOREPLACE: i32 = 0x100000;

    unsafe extern "C" {
        fn mmap(
            address: *mut c_void,
            length: usize,
            protection: i32,
            flags: i32,
            fd: i32,
            offset: isize,
        ) -> *mut c_void;
    }

    // SAFETY: requests a private anonymous fixed-address arena.
    let mapped = unsafe {
        mmap(
            ARENA_BASE as *mut c_void,
            ARENA_SIZE,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE,
            -1,
            0,
        )
    };
    if mapped as isize == -1 {
        Err(InitError::AddressUnavailable)
    } else {
        Ok(mapped.cast())
    }
}

#[cfg(not(any(windows, all(target_os = "linux", target_arch = "x86_64"))))]
fn mapping_exists() -> bool {
    false
}

#[cfg(not(any(windows, all(target_os = "linux", target_arch = "x86_64"))))]
fn map_arena() -> Result<*mut u8, InitError> {
    Err(InitError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn incompatible_provider_cannot_reinitialize_a_live_arena() {
        let _test = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        initialize().unwrap();
        let block = unsafe { malloc(64) };
        assert!(!block.is_null());
        unsafe { block.write(0x5a) };
        let bump = unsafe { (*header()).bump };
        struct RestoreMagic;
        impl Drop for RestoreMagic {
            fn drop(&mut self) {
                unsafe { (*header()).magic = ARENA_MAGIC };
            }
        }
        let restore = RestoreMagic;
        unsafe { (*header()).magic = ARENA_MAGIC ^ 1 };
        // This is the first-use path taken by a separately linked provider,
        // independent of this test module's already initialized fast path.
        assert_eq!(
            initialize_arena_mapping(),
            Err(InitError::IncompatibleLayout)
        );
        assert_eq!(unsafe { (*header()).bump }, bump);
        assert_eq!(unsafe { block.read() }, 0x5a);
        drop(restore);
        unsafe { free(block) };
    }

    #[cfg(windows)]
    #[test]
    fn mapping_transaction_is_reentrant_for_one_os_thread() {
        let _test = TEST_LOCK.lock().unwrap();
        let outer = begin_shared_mappings_transaction().expect("outer mapping transaction");
        let inner = begin_shared_mappings_transaction().expect("nested mapping transaction");
        drop(outer);
        // Dropping guards out of lexical order must retain ownership until the
        // final guard goes away.
        let third = begin_shared_mappings_transaction().expect("third mapping transaction");
        drop(inner);
        drop(third);
        let final_guard =
            begin_shared_mappings_transaction().expect("released mapping transaction");
        drop(final_guard);
    }

    #[test]
    fn mapping_registration_splits_replaced_placeholder_pages() {
        let mut mappings = [SharedForkMapping::default(); 4];
        let mut count = 0;
        assert!(replace_mapping_range(
            &mut mappings,
            &mut count,
            SharedForkMapping {
                base: 0x10000,
                len: 0x40000,
                behavior: 1,
                storage: 1,
                ..SharedForkMapping::default()
            },
        ));
        assert!(replace_mapping_range(
            &mut mappings,
            &mut count,
            SharedForkMapping {
                base: 0x4f000,
                len: 0x1000,
                behavior: 1,
                storage: 2,
                backing_slot: 0x9000,
                ..SharedForkMapping::default()
            },
        ));
        mappings[..count].sort_unstable_by_key(|entry| entry.base);
        assert_eq!(count, 2);
        assert_eq!(
            (mappings[0].base, mappings[0].len, mappings[0].storage),
            (0x10000, 0x3f000, 1)
        );
        assert_eq!(
            (mappings[1].base, mappings[1].len, mappings[1].storage),
            (0x4f000, 0x1000, 2)
        );
    }

    #[test]
    fn mapping_registration_preserves_section_offsets_when_split() {
        let mut mappings = [SharedForkMapping::default(); 4];
        let mut count = 1;
        mappings[0] = SharedForkMapping {
            base: 0x20000,
            len: 0x5000,
            behavior: 1,
            storage: 2,
            backing_slot: 0x1234,
            backing_offset: 0x8000,
            view_protection: 0,
            domain: 1,
        };
        assert!(replace_mapping_range(
            &mut mappings,
            &mut count,
            SharedForkMapping {
                base: 0x22000,
                len: 0x1000,
                behavior: 1,
                storage: 0,
                ..SharedForkMapping::default()
            },
        ));
        mappings[..count].sort_unstable_by_key(|entry| entry.base);
        assert_eq!(count, 3);
        assert_eq!(
            mappings[..count]
                .iter()
                .map(|m| m.domain)
                .collect::<Vec<_>>(),
            [1, 0, 1]
        );
        assert_eq!(
            (
                mappings[0].base,
                mappings[0].len,
                mappings[0].backing_offset
            ),
            (0x20000, 0x2000, 0x8000)
        );
        assert_eq!(
            (mappings[1].base, mappings[1].len, mappings[1].storage),
            (0x22000, 0x1000, 0)
        );
        assert_eq!(
            (
                mappings[2].base,
                mappings[2].len,
                mappings[2].backing_offset
            ),
            (0x23000, 0x2000, 0xb000)
        );
    }

    #[test]
    fn retained_sections_preserve_offsets_and_access_when_trimmed() {
        for replacement in [0x20000, 0x22000] {
            let mut mappings = [SharedForkMapping::default(); 4];
            let mut count = 1;
            mappings[0] = SharedForkMapping {
                base: 0x20000,
                len: 0x5000,
                behavior: 1,
                storage: 3,
                backing_slot: 0x1234,
                backing_offset: 0x8000,
                view_protection: 4,
                domain: 1,
            };
            assert!(replace_mapping_range(
                &mut mappings,
                &mut count,
                SharedForkMapping {
                    base: replacement,
                    len: 0x1000,
                    behavior: 1,
                    ..SharedForkMapping::default()
                }
            ));
            let right = mappings[..count]
                .iter()
                .find(|m| m.base == replacement + 0x1000)
                .unwrap();
            assert_eq!(
                right.backing_offset,
                0x8000 + (replacement + 0x1000 - 0x20000) as u64
            );
            assert_eq!(
                (right.storage, right.backing_slot, right.view_protection),
                (3, 0x1234, 4)
            );
        }
    }

    #[test]
    fn allocation_contents_and_free_list_survive_round_trip() {
        let _test = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        initialize().expect("fixed arena must be available to the test process");
        let first = unsafe { malloc(64) };
        assert!(!first.is_null());
        unsafe {
            first.write_bytes(0x5a, 64);
            assert_eq!(*first.add(31), 0x5a);
            free(first);
        }

        let reused = unsafe { malloc(64) };
        assert_eq!(reused, first, "an exact free-list block should be reused");
        unsafe { free(reused) };
    }

    #[test]
    fn retiring_workers_reuse_blocks_without_retiring_live_results() {
        let _test = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        initialize().unwrap();
        let before = snapshot().unwrap().used_len;
        let mut retained = Vec::new();
        for round in 0..32u8 {
            retained.push(
                std::thread::spawn(move || {
                    let result = unsafe { malloc(73) };
                    assert!(!result.is_null());
                    unsafe { result.write_bytes(round, 73) };
                    let mut blocks = [ptr::null_mut(); 64];
                    for block in &mut blocks {
                        *block = unsafe { malloc(4096) };
                        assert!(!block.is_null());
                        unsafe { block.write_bytes(0xa5, 4096) };
                    }
                    for block in blocks {
                        assert_eq!(unsafe { block.add(4095).read() }, 0xa5);
                        unsafe { free(block) };
                    }
                    release_thread_cache();
                    // A later destructor may still allocate on the same OS thread.
                    let scratch = unsafe { malloc(4096) };
                    assert!(!scratch.is_null());
                    unsafe { free(scratch) };
                    release_thread_cache();
                    result as usize
                })
                .join()
                .unwrap(),
            );
        }
        // The workers repeatedly use the same shared blocks instead of leaving
        // 256 KiB of otherwise reusable memory behind in every retired shard.
        assert!(snapshot().unwrap().used_len - before < 1024 * 1024);
        for (round, result) in retained.into_iter().enumerate() {
            let result = result as *mut u8;
            for offset in 0..73 {
                assert_eq!(unsafe { result.add(offset).read() }, round as u8);
            }
            unsafe { free(result) };
        }
    }

    #[test]
    fn cache_retirement_coexists_with_shared_shards_and_fork_freeze() {
        let _test = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        initialize().unwrap();
        let start = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut workers = Vec::new();
        for marker in [0x35, 0xa9] {
            let start = start.clone();
            workers.push(std::thread::spawn(move || {
                // More than 256 lifetime thread IDs can refer to one shard.
                LOCAL_CACHE_INDEX.with(|index| index.set(LOCAL_CACHE_SHARDS - 1));
                start.wait();
                for _ in 0..400 {
                    let block = unsafe { malloc(64 * 1024) };
                    assert!(!block.is_null());
                    unsafe { block.write_bytes(marker, 257) };
                    release_thread_cache();
                    for offset in 0..257 {
                        assert_eq!(unsafe { block.add(offset).read() }, marker);
                    }
                    unsafe { free(block) };
                }
                release_thread_cache();
            }));
        }
        start.wait();
        for _ in 0..100 {
            let (guard, snapshot) = freeze_snapshot().unwrap();
            assert!(snapshot.used_len <= snapshot.committed_len);
            drop(guard);
        }
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn size_classes_are_dense_then_geometric() {
        assert_eq!(free_bin_index(1), 0);
        assert_eq!(free_bin_index(16), 0);
        assert_eq!(free_bin_index(17), 1);
        assert_eq!(free_bin_index(128), LINEAR_CLASS_COUNT - 1);
        assert_eq!(free_bin_index(129), LINEAR_CLASS_COUNT);
        assert_eq!(free_bin_index(144), LINEAR_CLASS_COUNT);
        assert_eq!(free_bin_index(145), LINEAR_CLASS_COUNT + 1);
        assert_eq!(free_bin_index(4_097), free_bin_index(4_096) + 1);
        assert_eq!(free_bin_index(8_193), free_bin_index(8_192) + 1);
        assert_eq!(free_bin_index(usize::MAX), FREE_LIST_BIN_COUNT - 1);
    }

    #[test]
    fn allocation_capacities_are_canonical_within_each_size_class() {
        assert_eq!(allocation_capacity(1), Some(16));
        assert_eq!(allocation_capacity(129), Some(144));
        assert_eq!(allocation_capacity(4_096), Some(4_096));
        assert_eq!(allocation_capacity(4_097), Some(4_608));
        assert_eq!(allocation_capacity(8_192), Some(8_192));
        assert_eq!(allocation_capacity(8_193), Some(9_216));
        assert_eq!(allocation_capacity(ARENA_SIZE + 1), None);
    }

    #[test]
    fn large_requests_in_one_class_reuse_the_same_block() {
        let _test = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        initialize().expect("fixed arena must be available to the test process");
        let first = unsafe { malloc(4_097) };
        assert!(!first.is_null());
        unsafe { free(first) };

        let reused = unsafe { malloc(4_500) };
        assert_eq!(
            reused, first,
            "the canonical 4.5 KiB class should be reused"
        );
        unsafe { free(reused) };
    }

    #[test]
    fn large_tail_release_reuses_space_across_size_classes_and_alignment() {
        let _test = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        initialize().unwrap();
        let before = unsafe { (*header()).bump };
        for size in [7_000_123, 11_000_321, 17_000_999] {
            let layout = Layout::from_size_align(size, 65536).unwrap();
            let block = unsafe { allocate_from(header(), layout) };
            assert!(!block.is_null());
            assert_eq!(block as usize % 65536, 0);
            unsafe {
                block.write(0xa7);
                block.add(size - 1).write(0x73);
                deallocate_from(header(), block);
                assert_eq!((*header()).bump, before);
            }
        }
    }

    #[test]
    fn large_non_tail_release_preserves_live_successors() {
        let _test = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        initialize().unwrap();
        unsafe {
            let first = malloc(5_000_003);
            let guard = malloc(9_000_007);
            assert!(!first.is_null() && !guard.is_null());
            guard.write(0x67);
            guard.add(9_000_006).write(0xab);
            let before = (*header()).bump;
            free(first);
            assert_eq!((*header()).bump, before);
            let reused = malloc(5_000_003);
            assert_eq!(reused, first);
            assert_eq!(guard.read(), 0x67);
            assert_eq!(guard.add(9_000_006).read(), 0xab);
            free(guard);
            free(reused);
        }
    }

    #[test]
    fn mixed_size_allocations_are_safe_under_contention() {
        let _test = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        initialize().expect("fixed arena must be available to the test process");
        const SIZES: [usize; 10] = [16, 24, 64, 96, 128, 255, 512, 1_024, 4_096, 8_192];

        let workers = (0..8)
            .map(|worker| {
                std::thread::spawn(move || {
                    for round in 0..500 {
                        let mut blocks = [ptr::null_mut(); 32];
                        for (slot, block) in blocks.iter_mut().enumerate() {
                            let size = SIZES[(worker + round + slot) % SIZES.len()];
                            *block = unsafe { malloc(size) };
                            assert!(!block.is_null());
                            unsafe {
                                block.write(worker as u8);
                                block.add(size - 1).write(round as u8);
                            }
                        }
                        for (slot, block) in blocks.into_iter().enumerate().rev() {
                            let size = SIZES[(worker + round + slot) % SIZES.len()];
                            unsafe {
                                assert_eq!(block.read(), worker as u8);
                                assert_eq!(block.add(size - 1).read(), round as u8);
                                free(block);
                            }
                        }
                    }
                })
            })
            .collect::<Vec<_>>();

        // Cold refill must use the same lock order as fork's frozen snapshot.
        for _ in 0..128 {
            let (guard, _) = freeze_snapshot().unwrap();
            drop(guard);
            std::thread::yield_now();
        }
        for worker in workers {
            worker.join().expect("allocation worker panicked");
        }
    }

    #[test]
    fn batched_reuse_preserves_alignment_contents_and_arena_high_water() {
        let _test = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        initialize().unwrap();
        let layout = Layout::from_size_align(384, 256).unwrap();
        let mut blocks = [ptr::null_mut(); 2048];
        let mut high_water = 0;
        for round in 0..12 {
            for (index, block) in blocks.iter_mut().enumerate() {
                *block = unsafe { ManagedAllocator.alloc(layout) };
                assert!(!block.is_null());
                assert_eq!(*block as usize % 256, 0);
                unsafe {
                    block.cast::<usize>().write(index);
                    block.add(383).write(round);
                }
            }
            let unique: std::collections::HashSet<_> = blocks.iter().copied().collect();
            assert_eq!(unique.len(), blocks.len());
            let (guard, snapshot) = freeze_snapshot().unwrap();
            if round == 0 {
                high_water = snapshot.used_len;
            } else {
                assert_eq!(
                    snapshot.used_len, high_water,
                    "free blocks must be reusable"
                );
            }
            drop(guard);
            for (index, block) in blocks.iter().enumerate().rev() {
                unsafe {
                    assert_eq!(block.cast::<usize>().read(), index);
                    assert_eq!(block.add(383).read(), round);
                    ManagedAllocator.dealloc(*block, layout);
                }
            }
        }
    }

    #[test]
    fn cross_thread_frees_remain_reusable_and_bounded() {
        let _test = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        initialize().expect("fixed arena must be available to the test process");
        let blocks = (0..2_048)
            .map(|index| {
                let size = 16 + (index % 256) * 16;
                let block = unsafe { malloc(size) };
                assert!(!block.is_null());
                unsafe {
                    block.write((index & 0xff) as u8);
                    block.add(size - 1).write((index >> 3) as u8);
                }
                (block as usize, size)
            })
            .collect::<Vec<_>>();
        std::thread::spawn(move || {
            for (block, size) in blocks {
                let block = block as *mut u8;
                unsafe {
                    assert_eq!(block.read(), ((size - 16) / 16 & 0xff) as u8);
                    free(block);
                }
            }
        })
        .join()
        .expect("cross-thread free worker panicked");

        for index in 0..2_048 {
            let size = 16 + (index % 256) * 16;
            let block = unsafe { malloc(size) };
            assert!(!block.is_null());
            unsafe { free(block) };
        }
    }
}
