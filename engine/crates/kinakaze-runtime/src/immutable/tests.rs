use super::*;
use std::sync::atomic::Ordering;
use windows_sys::Win32::{Foundation::*, System::Memory::*};

fn query(base: *const u8) -> MEMORY_BASIC_INFORMATION {
    let mut info = unsafe { core::mem::zeroed() };
    assert_ne!(
        unsafe { VirtualQuery(base.cast(), &mut info, core::mem::size_of_val(&info)) },
        0
    );
    info
}

#[test]
fn snapshot_is_readonly_outside_arena_and_releases_its_native_ownership() {
    let _transaction = crate::begin_fork_mapping_transaction().unwrap();
    drop(ImmutableBytes::from_slice(b"warm registration").unwrap());
    let before = kinakaze_alloc::snapshot().unwrap().used_len;
    let bytes = ImmutableBytes::initialize(8 * 1024 * 1024 + 37, |target| {
        for (index, byte) in target.iter_mut().enumerate() {
            *byte = index.wrapping_mul(31) as u8;
        }
        Ok(())
    })
    .unwrap();
    assert!(kinakaze_alloc::snapshot().unwrap().used_len - before < 65536);
    let base = bytes.as_ptr();
    let info = query(base);
    assert_eq!(info.Type, MEM_MAPPED);
    assert_eq!(info.Protect, PAGE_READONLY);
    assert!(
        bytes
            .iter()
            .enumerate()
            .all(|(index, &byte)| byte == index.wrapping_mul(31) as u8)
    );
    let mapping = kinakaze_alloc::collect_shared_mappings()
        .into_iter()
        .find(|m| m.base == base as usize)
        .unwrap();
    assert_eq!(
        mapping.storage,
        crate::ForkMappingStorage::RetainedSection as u32
    );
    assert_eq!(mapping.domain, crate::ForkMappingDomain::HostPrivate as u32);
    let handle = unsafe { (*bytes.slot).load(Ordering::Acquire) } as _;
    let mut flags = 0;
    assert_ne!(unsafe { GetHandleInformation(handle, &mut flags) }, 0);
    assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
    // A second view pins the backing after the first owner unloads it.
    let alias = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, bytes.len()) };
    assert!(!alias.Value.is_null());
    drop(bytes);
    assert_eq!(query(base).State, MEM_FREE);
    assert!(
        !kinakaze_alloc::collect_shared_mappings()
            .iter()
            .any(|m| m.base == base as usize)
    );
    assert_eq!(unsafe { GetHandleInformation(handle, &mut flags) }, 0);
    assert_eq!(unsafe { alias.Value.cast::<u8>().add(65537).read() }, 31);
    assert_ne!(unsafe { UnmapViewOfFile(alias) }, 0);
}

#[test]
fn initialization_failure_unmaps_staging_and_does_not_publish() {
    let _transaction = crate::begin_fork_mapping_transaction().unwrap();
    let before = kinakaze_alloc::collect_shared_mappings().len();
    let mut base = core::ptr::null();
    let error = ImmutableBytes::initialize(8193, |bytes| {
        base = bytes.as_ptr();
        bytes.fill(7);
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "incomplete snapshot",
        ))
    })
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    assert_eq!(query(base).State, MEM_FREE);
    assert_eq!(kinakaze_alloc::collect_shared_mappings().len(), before);
    assert!(ImmutableBytes::from_slice(&[]).unwrap().is_empty());
    assert!(
        ImmutableBytes::initialize(usize::MAX, |_| panic!(
            "overflow must reject before initialization"
        ))
        .is_err()
    );
}
