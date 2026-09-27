//! Prepare independent anonymous views while the caller owns VMA topology.
//! Native work uses no guest state; every temporary thread joins before publish.
use super::*;
use windows_sys::Win32::System::Memory::{
    MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile3, UnmapViewOfFile2,
};

const CHUNK: usize = 16 * 1024 * 1024;

struct View {
    address: usize,
    length: usize,
    section: usize,
    mapped: bool,
    error: u32,
}

impl View {
    fn prepare(&mut self, protection: u32) {
        let section = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_EXECUTE_READWRITE,
                0,
                self.length as u32,
                ptr::null(),
            )
        };
        if section.is_null() {
            self.error = unsafe { GetLastError() };
            return;
        }
        self.section = section as usize;
        let view = unsafe {
            MapViewOfFile3(
                section,
                GetCurrentProcess(),
                self.address as _,
                0,
                self.length,
                MEM_REPLACE_PLACEHOLDER,
                PAGE_EXECUTE_READWRITE,
                ptr::null_mut(),
                0,
            )
        };
        if view.Value.is_null() {
            self.error = unsafe { GetLastError() };
            return;
        }
        self.mapped = true;
        let mut old = 0;
        if unsafe { VirtualProtect(view.Value, self.length, protection, &mut old) } == 0 {
            self.error = unsafe { GetLastError() };
        }
    }
}

impl Drop for View {
    fn drop(&mut self) {
        // Until publication the registry still describes placeholders. Restore
        // that exact topology on any native/allocation/unwind failure.
        if self.mapped
            && unsafe {
                UnmapViewOfFile2(
                    GetCurrentProcess(),
                    MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: self.address as _,
                    },
                    MEM_PRESERVE_PLACEHOLDER,
                )
            } == 0
        {
            std::process::abort();
        }
        if self.section != 0 {
            unsafe { CloseHandle(self.section as _) };
        }
    }
}

pub(super) fn replace(address: *mut c_void, length: usize, protection: u32) -> Result<bool, i32> {
    if length < 4 * CHUNK {
        return Ok(false);
    }
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(4);
    if threads < 2 {
        return Ok(false);
    }
    let start = address as usize;
    let end = start.checked_add(length).ok_or(EINVAL)?;
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;
    let mut registry = mappings().lock().map_err(|_| EIO)?;
    if !registry.iter().any(|(&base, mapping)| {
        mapping.kind == MappingKind::Placeholder
            && base <= start
            && base
                .checked_add(mapping.length)
                .is_some_and(|limit| end <= limit)
    }) {
        return Ok(false);
    }
    let mut views: Vec<_> = (0..length)
        .step_by(CHUNK)
        .map(|offset| View {
            address: start + offset,
            length: CHUNK.min(length - offset),
            section: 0,
            mapped: false,
            error: 0,
        })
        .collect();
    for view in &views {
        if !carve_placeholder_locked(&mut registry, view.address, view.length)? {
            return Err(EIO);
        }
    }
    let per_thread = views.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let mut groups = views.chunks_mut(per_thread);
        let first = groups.next().unwrap();
        let mut workers = Vec::new();
        for group in groups {
            // A failed spawn leaves that group's zero handles for the serial
            // fallback after all other workers have joined.
            if let Ok(worker) = std::thread::Builder::new()
                .name("anonymous-map".into())
                .spawn_scoped(scope, move || {
                    for view in group {
                        view.prepare(protection);
                    }
                })
            {
                workers.push(worker);
            }
        }
        for view in first {
            view.prepare(protection);
        }
        for worker in workers {
            if worker.join().is_err() {
                return Err(EIO);
            }
        }
        Ok(())
    })?;
    for view in &mut views {
        if view.section == 0 && view.error == 0 {
            view.prepare(protection);
        }
    }
    if let Some(view) = views.iter().find(|view| view.error != 0) {
        return Err(errno_from_win32(view.error));
    }
    // Managed allocations/handle registration stay on the transaction owner.
    // Finish all fallible preparation before publishing even the first view.
    let mut backings = Vec::with_capacity(views.len());
    for view in &mut views {
        let backing = BackingRef::new_snapshot(view.section as _, view.length)?;
        view.section = 0;
        backings.push(backing);
    }
    for (view, backing) in views.iter_mut().zip(backings) {
        let placeholder = registry.remove(&view.address).ok_or(EIO)?;
        unregister_fork_fragment(view.address, &placeholder);
        insert_mapping_fragment(
            &mut registry,
            view.address,
            Mapping {
                base: view.address as _,
                length: view.length,
                kind: MappingKind::PlaceholderView,
                shared: false,
                backing: Some(backing),
                backing_offset: 0,
                view_protection: protection,
                file: None,
                verity: None,
                native_inode: None,
                file_origin: None,
            },
        );
        view.mapped = false;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_parallel_protection_restores_placeholders_and_allows_retry() {
        if std::thread::available_parallelism().map_or(1, |n| n.get()) < 2 {
            return;
        }
        let length = 4 * CHUNK + memory_geometry().0;
        let address = map(
            ptr::null_mut(),
            length,
            PROT_NONE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        )
        .unwrap();
        assert!(replace(address, length, 0xdead_beef).is_err());
        for offset in (0..length).step_by(CHUNK) {
            let mut info: MemoryBasicInformation = unsafe { core::mem::zeroed() };
            assert_ne!(
                unsafe {
                    VirtualQuery(
                        address.byte_add(offset),
                        &mut info,
                        size_of::<MemoryBasicInformation>(),
                    )
                },
                0
            );
            assert_eq!(info.state, MEM_RESERVE_STATE);
            assert_eq!(
                mappings()
                    .lock()
                    .unwrap()
                    .get(&(address as usize + offset))
                    .unwrap()
                    .kind,
                MappingKind::Placeholder
            );
        }
        // A failed attempt may split the reservation; the ordinary replacement
        // entry must still commit those exact fragments successfully.
        for offset in (0..length).step_by(CHUNK) {
            let at = unsafe { address.byte_add(offset) };
            let bytes = CHUNK.min(length - offset);
            assert_eq!(
                map(
                    at,
                    bytes,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                    -1,
                    0
                )
                .unwrap(),
                at
            );
            assert_eq!(unsafe { at.cast::<u8>().read() }, 0);
            unsafe { at.cast::<u8>().write(0x73) };
        }
        unmap(address, length).unwrap();
    }
}
