use super::*;

unsafe fn fixture() -> (ForkMapping, HANDLE) {
    let len = 256 * 1024;
    let section = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_EXECUTE_READWRITE,
            0,
            len as u32,
            ptr::null(),
        )
    };
    assert!(!section.is_null());
    let base = unsafe {
        VirtualAlloc2(
            GetCurrentProcess(),
            ptr::null(),
            len,
            MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
            PAGE_NOACCESS,
            ptr::null_mut(),
            0,
        )
    };
    assert!(!base.is_null());
    let view = unsafe {
        MapViewOfFile3(
            section,
            GetCurrentProcess(),
            base,
            0,
            len,
            MEM_REPLACE_PLACEHOLDER,
            PAGE_EXECUTE_WRITECOPY,
            ptr::null_mut(),
            0,
        )
    };
    assert_eq!(view.Value, base);
    let mapping = ForkMapping {
        base: base as usize,
        len,
        behavior: ForkMappingBehavior::Copy,
        storage: ForkMappingStorage::AnonymousSnapshot,
        backing_slot: 0,
        backing_offset: 0,
        view_protection: PAGE_EXECUTE_WRITECOPY,
        domain: ForkMappingDomain::GuestMm,
    };
    unsafe { ptr::write_bytes(base, 31, len) };
    (mapping, section)
}

#[test]
fn refreshed_heap_keeps_old_children_and_new_children_private() {
    unsafe {
        let (mapping, original) = fixture();
        let bytes = mapping.base as *mut u8;
        let old_child = MapViewOfFile(original, FILE_MAP_COPY, 0, 0, mapping.len);
        assert!(!old_child.Value.is_null());
        let mut previous = 0;
        assert_ne!(
            VirtualProtect(bytes.add(4096).cast(), 4096, PAGE_NOACCESS, &mut previous),
            0
        );
        assert_ne!(
            VirtualProtect(bytes.add(8192).cast(), 4096, PAGE_READONLY, &mut previous),
            0
        );
        let snapshot = Snapshot::capture(&mapping).unwrap();
        assert!(snapshot.replace(&mapping, original, snapshot.section));
        assert_eq!(query(mapping.base + 4096).unwrap().Protect, PAGE_NOACCESS);
        assert_eq!(query(mapping.base + 8192).unwrap().Protect, PAGE_READONLY);
        assert_eq!(bytes.read_volatile(), 31);
        assert_eq!(old_child.Value.cast::<u8>().read_volatile(), 0);
        let new_child = MapViewOfFile(snapshot.section, FILE_MAP_COPY, 0, 0, mapping.len);
        assert!(!new_child.Value.is_null());
        let next = new_child.Value.cast::<u8>();
        assert_eq!(next.read_volatile(), 31);
        bytes.write_volatile(47);
        next.add(8192).write_volatile(53);
        assert_eq!(next.read_volatile(), 31);
        assert_eq!(bytes.add(8192).read_volatile(), 31);
        assert_eq!(snapshot.staging.Value.cast::<u8>().read_volatile(), 31);
        let second = Snapshot::capture(&mapping).unwrap();
        assert!(second.replace(&mapping, snapshot.section, second.section));
        assert_eq!(bytes.read_volatile(), 47);
        assert_eq!(next.read_volatile(), 31);
        assert_eq!(old_child.Value.cast::<u8>().read_volatile(), 0);
        UnmapViewOfFile(new_child);
        UnmapViewOfFile(old_child);
        UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
            Value: bytes.cast(),
        });
        CloseHandle(original);
    }
}

#[test]
fn failed_replacement_restores_private_bytes_and_protection() {
    unsafe {
        let (mapping, original) = fixture();
        let bytes = mapping.base as *mut u8;
        let child = MapViewOfFile(original, FILE_MAP_COPY, 0, 0, mapping.len);
        assert!(!child.Value.is_null());
        bytes.add(mapping.len - 1).write_volatile(71);
        let mut previous = 0;
        assert_ne!(
            VirtualProtect(bytes.add(4096).cast(), 4096, PAGE_NOACCESS, &mut previous),
            0
        );
        let snapshot = Snapshot::capture(&mapping).unwrap();
        assert!(!snapshot.replace(&mapping, original, ptr::null_mut()));
        assert_eq!(query(mapping.base + 4096).unwrap().Protect, PAGE_NOACCESS);
        assert_eq!(bytes.read_volatile(), 31);
        assert_eq!(bytes.add(mapping.len - 1).read_volatile(), 71);
        assert_eq!(child.Value.cast::<u8>().read_volatile(), 0);
        assert!(snapshot.replace(&mapping, original, snapshot.section));
        assert_eq!(bytes.add(mapping.len - 1).read_volatile(), 71);
        UnmapViewOfFile(child);
        UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
            Value: bytes.cast(),
        });
        CloseHandle(original);
    }
}

#[test]
fn refresh_rejects_partial_views_and_the_running_stack() {
    unsafe {
        let (mut mapping, section) = fixture();
        let length = mapping.len;
        mapping.len = 4096;
        assert!(Snapshot::capture(&mapping).is_none());
        mapping.len = length;
        UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
            Value: mapping.base as _,
        });
        CloseHandle(section);
        let stack = 0u8;
        mapping.base = ptr::addr_of!(stack) as usize & !4095;
        mapping.len = 4096;
        assert!(Snapshot::capture(&mapping).is_none());
    }
}
