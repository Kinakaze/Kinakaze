use super::*;
use kinakaze_runtime::memory_protection::protect_preserving_copy_on_write as protect;
use std::sync::atomic::Ordering;
use windows_sys::Win32::System::Memory::*;

#[test]
fn elf_initialization_and_later_write_permissions_keep_the_backing_pristine() {
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().unwrap();
    let bytes = vec![17; 3 * 4096];
    let header = ProgramHeader {
        kind: kinakaze_elf::PT_LOAD,
        flags: 7,
        offset: 0,
        virtual_address: 4096,
        file_size: bytes.len() as u64,
        memory_size: (bytes.len() + 4096) as u64,
        align: 4096,
    };
    let snapshot = crate::ImmutableBytes::from_slice(&bytes).unwrap();
    let mapping = ImageMapping::new(false, 0, 65536, "COW test", &snapshot, &[header]).unwrap();
    unsafe {
        let handle = (*mapping.slot).load(Ordering::Acquire) as _;
        let original = MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 65536);
        assert!(!original.Value.is_null());
        let original_bytes = original.Value.cast::<u8>();
        assert_eq!(mapping.base.read_volatile(), 0); // gap before PT_LOAD
        assert_eq!(mapping.base.add(16384).read_volatile(), 0); // BSS
        let mut old = 0;
        // A clean code page, a readonly data page and a no-access data page.
        for (index, initial) in [PAGE_EXECUTE_READ, PAGE_READONLY, PAGE_NOACCESS]
            .into_iter()
            .enumerate()
        {
            let offset = (index + 1) * 4096;
            let address = mapping.base.add(offset);
            assert_ne!(protect(address.cast(), 4096, initial, &mut old), 0);
            let write = if index == 0 {
                PAGE_EXECUTE_READWRITE
            } else {
                PAGE_READWRITE
            };
            assert_ne!(protect(address.cast(), 4096, write, &mut old), 0);
            assert_eq!(address.read_volatile(), 17);
            address.write_volatile(31 + index as u8);
            assert_eq!(original_bytes.add(offset).read_volatile(), 17);
            // AllocationProtect must identify private ownership even after the
            // first write fault changes the per-page Protect reported by Windows.
            assert_ne!(protect(address.cast(), 4096, PAGE_NOACCESS, &mut old), 0);
            assert_ne!(protect(address.cast(), 4096, write, &mut old), 0);
            address.write_volatile(43);
            assert_eq!(original_bytes.add(offset).read_volatile(), 17);
        }
        let base = mapping.base;
        drop(mapping);
        let mut info: MEMORY_BASIC_INFORMATION = core::mem::zeroed();
        assert_ne!(
            VirtualQuery(base.cast(), &mut info, core::mem::size_of_val(&info)),
            0
        );
        assert_eq!(info.State, MEM_FREE);
        assert_eq!(original_bytes.add(4096).read_volatile(), 17);
        assert_ne!(UnmapViewOfFile(original), 0);
    }
}

#[test]
fn shared_snapshot_matches_segment_copy_and_keeps_source_and_ownership_private() {
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().unwrap();
    let len = 2 * 1024 * 1024 + 256 * 1024;
    let data: Vec<_> = (0..len + 37).map(|i| (i % 251) as u8).collect();
    let snapshot = crate::ImmutableBytes::initialize_executable(data.len(), |bytes| {
        bytes.copy_from_slice(&data);
        Ok(())
    })
    .unwrap();
    let headers = [
        ProgramHeader {
            kind: kinakaze_elf::PT_LOAD,
            flags: 5,
            offset: 0,
            virtual_address: 0,
            file_size: 2 * 1024 * 1024 + 37,
            memory_size: 2 * 1024 * 1024 + 37,
            align: 4096,
        },
        ProgramHeader {
            kind: kinakaze_elf::PT_LOAD,
            flags: 6,
            offset: 2 * 1024 * 1024 + 8192 + 17,
            virtual_address: 2 * 1024 * 1024 + 12288 + 17,
            file_size: 128 * 1024 + 13,
            memory_size: 128 * 1024 + 13 + 8192,
            align: 4096,
        },
        ProgramHeader {
            kind: kinakaze_elf::PT_LOAD,
            flags: 6,
            offset: 0,
            virtual_address: 2 * 1024 * 1024 + 192 * 1024,
            file_size: 0,
            memory_size: 32768,
            align: 4096,
        },
    ];
    let mut expected = vec![0; len];
    super::super::copy_segments(&data, &headers, expected.as_mut_ptr() as usize, 0, len).unwrap();
    let mapping =
        ImageMapping::from_snapshot(false, 0, len, "shared snapshot", &snapshot, &headers)
            .unwrap()
            .expect("eligible executable snapshot must use its section");
    unsafe {
        assert_eq!(core::slice::from_raw_parts(mapping.base, len), expected);
        assert_eq!(snapshot.as_slice(), data);
        let mut info: MEMORY_BASIC_INFORMATION = core::mem::zeroed();
        assert_ne!(
            VirtualQuery(
                mapping.base.cast(),
                &mut info,
                core::mem::size_of_val(&info)
            ),
            0
        );
        assert_eq!(info.AllocationProtect, PAGE_EXECUTE_WRITECOPY);
        let mut old = 0;
        assert_ne!(
            protect(
                mapping.base.add(65536).cast(),
                4096,
                PAGE_READWRITE,
                &mut old
            ),
            0
        );
        mapping.base.add(65536).write_volatile(251);
        assert_eq!(
            snapshot.as_slice(),
            data,
            "private writes changed the immutable input"
        );
        expected[65536] = 251;
        // The execution owner keeps its section alive after the source unloads.
        drop(snapshot);
        assert_eq!(core::slice::from_raw_parts(mapping.base, len), expected);
        let base = mapping.base;
        drop(mapping);
        assert_ne!(
            VirtualQuery(base.cast(), &mut info, core::mem::size_of_val(&info)),
            0
        );
        assert_eq!(info.State, MEM_FREE);
    }
}

#[test]
fn incompatible_snapshots_keep_the_general_copy_path_and_release_failed_attempts() {
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().unwrap();
    let len = 2 * 1024 * 1024;
    let data = vec![93; len];
    let snapshot = crate::ImmutableBytes::from_slice(&data).unwrap(); // data-only section
    let mut headers = [ProgramHeader {
        kind: kinakaze_elf::PT_LOAD,
        flags: 5,
        offset: 0,
        virtual_address: 0,
        file_size: len as u64,
        memory_size: len as u64,
        align: 4096,
    }];
    let count = kinakaze_alloc::collect_shared_mappings().len();
    assert!(
        ImageMapping::from_snapshot(false, 0, len, "data-only", &snapshot, &headers)
            .unwrap()
            .is_none()
    );
    assert_eq!(kinakaze_alloc::collect_shared_mappings().len(), count);
    let mapping = ImageMapping::new(false, 0, len, "fallback", &snapshot, &headers).unwrap();
    assert_eq!(
        unsafe { core::slice::from_raw_parts(mapping.base, len) },
        data
    );
    drop(mapping);
    headers[0].memory_size += 65536;
    assert!(snapshot_layout(data.len(), &headers, 0, len + 65536).is_none());
    let mapping =
        ImageMapping::new(false, 0, len + 65536, "large BSS", &snapshot, &headers).unwrap();
    assert!(
        unsafe { core::slice::from_raw_parts(mapping.base.add(len), 65536) }
            .iter()
            .all(|&b| b == 0)
    );
    let overlaps = [
        headers[0],
        ProgramHeader {
            virtual_address: 4096,
            offset: 4096,
            ..headers[0]
        },
    ];
    assert!(snapshot_layout(data.len(), &overlaps, 0, len).is_none());
    let sparse = [ProgramHeader {
        file_size: 1024 * 1024,
        memory_size: 3 * 1024 * 1024,
        ..headers[0]
    }];
    assert!(snapshot_layout(3 * 1024 * 1024, &sparse, 0, 3 * 1024 * 1024).is_none());
}

#[test]
fn shared_snapshot_supports_the_fixed_executable_address() {
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().unwrap();
    let len = 2 * 1024 * 1024;
    let snapshot = crate::ImmutableBytes::initialize_executable(len, |bytes| {
        bytes.fill(37);
        Ok(())
    })
    .unwrap();
    unsafe {
        let address = VirtualAlloc(core::ptr::null(), len, MEM_RESERVE, PAGE_NOACCESS);
        assert!(!address.is_null());
        assert_ne!(VirtualFree(address, 0, MEM_RELEASE), 0);
        let header = ProgramHeader {
            kind: kinakaze_elf::PT_LOAD,
            flags: 5,
            offset: 0,
            virtual_address: address as u64,
            file_size: len as u64,
            memory_size: len as u64,
            align: 4096,
        };
        let mapping = ImageMapping::from_snapshot(
            true,
            address as u64,
            len,
            "fixed shared",
            &snapshot,
            &[header],
        )
        .unwrap()
        .expect("fixed-address snapshot view");
        assert_eq!(mapping.base.cast(), address);
        assert!(
            core::slice::from_raw_parts(mapping.base, len)
                .iter()
                .all(|&b| b == 37)
        );
    }
}
