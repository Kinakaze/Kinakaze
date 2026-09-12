use super::*;
use crate::{ForkMappingBehavior, ForkMappingDomain, ForkMappingStorage};
use windows_sys::Win32::{
    Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
    System::Memory::{
        CreateFileMappingW, FILE_MAP_COPY, FILE_MAP_READ, MEM_RESERVE_PLACEHOLDER, MapViewOfFile,
        PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_NOACCESS, PAGE_READONLY,
        UnmapViewOfFile, VirtualAlloc2,
    },
};

#[test]
fn initialized_anonymous_snapshot_preserves_protection_and_private_generations() {
    unsafe {
        let len = 16 * 4096;
        let base = VirtualAlloc2(
            GetCurrentProcess(),
            ptr::null(),
            len + 65536,
            MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
            PAGE_NOACCESS,
            ptr::null_mut(),
            0,
        );
        assert!(!base.is_null());
        assert_ne!(
            VirtualFree(base, len, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER),
            0
        );
        let section = CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_EXECUTE_READWRITE,
            0,
            len as u32,
            ptr::null(),
        );
        assert!(!section.is_null());
        let parent = MapViewOfFile3(
            section,
            GetCurrentProcess(),
            base,
            0,
            len,
            MEM_REPLACE_PLACEHOLDER,
            PAGE_READWRITE,
            ptr::null_mut(),
            0,
        );
        assert_eq!(parent.Value, base);
        let bytes = base.cast::<u8>();
        for offset in (0..len).step_by(4096) {
            bytes.add(offset).write_volatile(17 + (offset / 4096) as u8);
        }
        let mut old = 0;
        assert_ne!(
            VirtualProtect(bytes.add(4096).cast(), 4096, PAGE_NOACCESS, &mut old),
            0
        );
        assert_ne!(
            VirtualProtect(bytes.add(8192).cast(), 4096, PAGE_READONLY, &mut old),
            0
        );
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
        assert!(snapshot(&mapping, section).unwrap());
        assert_eq!(
            query(base as usize).unwrap().AllocationProtect,
            PAGE_EXECUTE_WRITECOPY
        );
        assert_eq!(
            query(bytes.add(4096) as usize).unwrap().Protect,
            PAGE_NOACCESS
        );
        assert_eq!(
            query(bytes.add(8192) as usize).unwrap().Protect,
            PAGE_READONLY
        );
        let child = MapViewOfFile(section, FILE_MAP_COPY, 0, 0, len);
        let backing = MapViewOfFile(section, FILE_MAP_READ, 0, 0, len);
        assert!(!child.Value.is_null() && !backing.Value.is_null());
        let child_bytes = child.Value.cast::<u8>();
        for offset in (0..len).step_by(4096) {
            assert_eq!(
                child_bytes.add(offset).read_volatile(),
                17 + (offset / 4096) as u8
            );
        }
        bytes.write_volatile(73);
        child_bytes.add(4096).write_volatile(79);
        assert!(!snapshot(&mapping, section).unwrap());
        assert_eq!(child_bytes.read_volatile(), 17);
        assert_eq!(backing.Value.cast::<u8>().read_volatile(), 17);
        assert_ne!(
            crate::memory_protection::protect_preserving_copy_on_write(
                bytes.add(4096).cast(),
                4096,
                PAGE_READWRITE,
                &mut old
            ),
            0
        );
        assert_eq!(bytes.add(4096).read_volatile(), 18);
        bytes.add(4096).write_volatile(83);
        assert_eq!(child_bytes.add(4096).read_volatile(), 79);
        UnmapViewOfFile(child);
        UnmapViewOfFile(backing);
        assert_ne!(UnmapViewOfFile(parent), 0);
        assert_ne!(VirtualFree(bytes.add(len).cast(), 0, MEM_RELEASE), 0);
        CloseHandle(section);
    }
}

#[test]
fn rejects_active_stack_and_partial_view_before_unmapping() {
    unsafe {
        let stack = 0u8;
        let base = ptr::addr_of!(stack) as usize & !4095;
        let mut mapping = ForkMapping {
            base,
            len: 4096,
            behavior: ForkMappingBehavior::Copy,
            storage: ForkMappingStorage::AnonymousSnapshot,
            backing_slot: 0,
            backing_offset: 0,
            view_protection: PAGE_EXECUTE_WRITECOPY,
            domain: ForkMappingDomain::GuestMm,
        };
        assert!(snapshot(&mapping, ptr::null_mut()).is_err());
        let section = CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            0,
            65536,
            ptr::null(),
        );
        assert!(!section.is_null());
        let view = MapViewOfFile(section, 2, 0, 0, 65536);
        assert!(!view.Value.is_null());
        mapping.base = view.Value as usize;
        mapping.len = 4096;
        assert!(snapshot(&mapping, section).is_err());
        view.Value.cast::<u8>().write_volatile(97);
        assert_eq!(view.Value.cast::<u8>().read_volatile(), 97);
        UnmapViewOfFile(view);
        CloseHandle(section);
    }
}
