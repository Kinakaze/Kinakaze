use super::*;

#[test]
fn statistics_account_for_live_and_reusable_blocks() {
    let _test = super::super::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let block = unsafe { malloc(65536) };
    assert!(!block.is_null());
    let live = statistics();
    assert!(live.used >= 65536);
    assert_eq!(live.used + live.free, live.arena);
    unsafe {
        free(block);
    }
    let released = statistics();
    assert!(released.used < live.used);
    assert!(released.free_blocks > live.free_blocks);
    assert_eq!(released.used + released.free, released.arena);
}

#[test]
fn c_storage_is_separate_and_legacy_buffers_keep_their_owner() {
    let _test = super::super::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let host = unsafe { super::super::malloc(128) };
    assert!(!host.is_null());
    let guest = unsafe { memalign(4096, 3 * 1024 * 1024) };
    assert!(contains(guest as usize));
    assert!(!contains(host as usize));
    assert_eq!(guest as usize % 4096, 0);
    unsafe {
        host.write_bytes(0x5b, 128);
        guest.write_bytes(0x73, 3 * 1024 * 1024);
        let grown = reallocate(host, 16, 4096);
        assert!(contains(grown as usize));
        assert_eq!(core::slice::from_raw_parts(grown, 128), &[0x5b; 128]);
        assert_eq!(guest.add(3 * 1024 * 1024 - 1).read(), 0x73);
        free(grown);
        free(guest);
        let legacy = super::super::malloc(96);
        let capacity = usable_size(legacy);
        assert!(capacity >= 96);
        free(legacy);
    }
    let mappings = collect_shared_mappings();
    let entry = mappings.iter().find(|m| m.base == ARENA_BASE).unwrap();
    assert_eq!(entry.domain, 1);
    assert_eq!(entry.len, super::super::ARENA_SIZE);
}

#[test]
fn independent_free_lists_survive_parallel_growth_and_cross_thread_free() {
    let _test = super::super::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let workers: Vec<_> = (0..8)
        .map(|worker| {
            std::thread::spawn(move || {
                for round in 0..150 {
                    let size = if round % 31 == 0 {
                        2 * 1024 * 1024 + 1
                    } else {
                        32 + (round % 29) * 1024
                    };
                    let guest = unsafe { malloc(size) };
                    let host = unsafe { super::super::malloc(size / 8) };
                    assert!(contains(guest as usize));
                    assert!(!host.is_null());
                    assert!(!contains(host as usize));
                    unsafe {
                        guest.write_bytes(worker as u8 + 1, size);
                        host.write_bytes(0xa7, size / 8);
                        assert_eq!(guest.add(size - 1).read(), worker as u8 + 1);
                        free(guest);
                        free(host);
                    }
                }
                let last = unsafe { malloc(513) };
                assert!(!last.is_null());
                unsafe {
                    last.write_bytes(0x17, 513);
                }
                last as usize
            })
        })
        .collect();
    for worker in workers {
        let block = worker.join().unwrap() as *mut u8;
        unsafe {
            assert_eq!(block.add(512).read(), 0x17);
            free(block);
        }
    }
}

#[test]
fn frozen_header_repair_preserves_payload_metadata_and_parent_locks() {
    let _test = super::super::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    initialize().unwrap();
    // Use native scratch: the parent host allocator can be frozen as well.
    use windows_sys::Win32::System::Memory::*;
    let scratch = unsafe {
        VirtualAlloc(
            ptr::null(),
            HEADER_BYTES,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    assert!(!scratch.is_null());
    let frozen = freeze_if_initialized().unwrap();
    unsafe {
        ptr::copy_nonoverlapping(ARENA_BASE as *const u8, scratch.cast(), HEADER_BYTES);
        clear_copied_locks(scratch.cast());
        let copy = &*scratch.cast::<ArenaHeader>();
        let parent = &*(ARENA_BASE as *const ArenaHeader);
        assert_eq!(copy.magic, MAGIC);
        assert_eq!(copy.bump, parent.bump);
        assert_eq!(copy.committed, parent.committed);
        assert_eq!(copy.free_heads, parent.free_heads);
        assert_eq!(copy.lock.load(Ordering::Relaxed), 0);
        assert_ne!(parent.lock.load(Ordering::Relaxed), 0);
        assert!(
            copy.free_locks
                .iter()
                .all(|lock| lock.load(Ordering::Relaxed) == 0)
        );
        assert!(
            copy.local_caches
                .iter()
                .all(|cache| cache.lock.load(Ordering::Relaxed) == 0)
        );
    }
    drop(frozen);
    unsafe {
        VirtualFree(scratch, 0, MEM_RELEASE);
    }
}
