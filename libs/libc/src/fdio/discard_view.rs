//! Discard whole private section views without touching every zero-fill page.
use super::*;
use windows_sys::Win32::System::Memory::{
    MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile3, UnmapViewOfFile2,
};

/// Only replace an exact, non-COW, anonymous view. Partial views and forked
/// private pages retain the in-place path, which leaves neighbouring pages live.
/// The caller owns the fork topology transaction throughout.
pub(super) fn replace_whole(start: usize, length: usize, protection: u32) -> Result<bool, i32> {
    let mut registry = mappings().lock().map_err(|_| EIO)?;
    let Some(mapping) = registry
        .get(&start)
        .filter(|mapping| {
            mapping.base as usize == start
                && mapping.length == length
                && mapping.kind == MappingKind::PlaceholderView
                && !mapping.shared
                && mapping.file.is_none()
                && mapping.file_origin.is_none()
                && mapping.verity.is_none()
                && mapping.native_inode.is_none()
                && mapping.tmpfs.is_none()
                && mapping
                    .backing
                    .as_ref()
                    .is_some_and(|b| b.is_snapshot() || b.is_remappable())
        })
        .cloned()
    else {
        return Ok(false);
    };
    let mut info: MemoryBasicInformation = unsafe { core::mem::zeroed() };
    if unsafe { VirtualQuery(start as _, &mut info, size_of::<MemoryBasicInformation>()) } == 0 {
        return Err(last_errno());
    }
    if info.allocation_base as usize != start
        || info.region_size != length
        || !matches!(
            info.allocation_protect & 0xff,
            PAGE_READWRITE | PAGE_EXECUTE_READWRITE
        )
    {
        return Ok(false);
    }
    // Prepare every fallible allocation before parking sibling guest writers.
    let section = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_EXECUTE_READWRITE,
            (length as u64 >> 32) as u32,
            length as u32,
            ptr::null(),
        )
    };
    if section.is_null() {
        return Err(last_errno());
    }
    let backing = match BackingRef::new_snapshot(section, length) {
        Ok(backing) => backing,
        Err(error) => {
            unsafe { CloseHandle(section) };
            return Err(error);
        }
    };
    {
        let _frozen = unsafe { kinakaze_runtime::freeze_memory_threads() }.map_err(|_| EIO)?;
        let old = MEMORY_MAPPED_VIEW_ADDRESS { Value: start as _ };
        if unsafe { UnmapViewOfFile2(GetCurrentProcess(), old, MEM_PRESERVE_PLACEHOLDER) } == 0 {
            return Err(last_errno());
        }
        let view = unsafe {
            MapViewOfFile3(
                section,
                GetCurrentProcess(),
                start as _,
                0,
                length,
                MEM_REPLACE_PLACEHOLDER,
                PAGE_EXECUTE_READWRITE,
                ptr::null_mut(),
                0,
            )
        };
        let mut previous = 0;
        let installed = !view.Value.is_null();
        if !installed
            || unsafe { VirtualProtect(start as _, length, protection, &mut previous) } == 0
        {
            let error = last_errno();
            if installed
                && unsafe { UnmapViewOfFile2(GetCurrentProcess(), view, MEM_PRESERVE_PLACEHOLDER) }
                    == 0
            {
                std::process::abort();
            }
            let restored = unsafe {
                MapViewOfFile3(
                    mapping.backing.as_ref().unwrap().handle(),
                    GetCurrentProcess(),
                    start as _,
                    mapping.backing_offset,
                    length,
                    MEM_REPLACE_PLACEHOLDER,
                    info.allocation_protect,
                    ptr::null_mut(),
                    0,
                )
            };
            if restored.Value as usize != start
                || unsafe { VirtualProtect(start as _, length, protection, &mut previous) } == 0
            {
                std::process::abort();
            }
            return Err(error);
        }
    }
    // Replacing an existing entry cannot grow the registry. Drop the previous
    // backing only after the new view and its fork metadata are published.
    insert_mapping_fragment(
        &mut registry,
        start,
        Mapping {
            backing: Some(backing),
            backing_offset: 0,
            view_protection: protection,
            ..mapping
        },
    );
    Ok(true)
}
