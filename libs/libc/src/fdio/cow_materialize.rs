//! Materialize a COW view only when an operation must split its native view.
//! Sibling threads stay parked while bytes and the native address are replaced.
use super::*;
use windows_sys::Win32::System::{
    Diagnostics::Debug::{FlushInstructionCache, ReadProcessMemory, WriteProcessMemory},
    Memory::{MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile3, PAGE_EXECUTE, UnmapViewOfFile2},
};

struct Staging(*mut c_void);
impl Drop for Staging {
    fn drop(&mut self) {
        unsafe { UnmapViewOfFile(self.0) };
    }
}

fn protection(value: u32, cow: bool) -> u32 {
    (value & !0xff)
        | match (value & 0xff, cow) {
            (PAGE_WRITECOPY, false) => PAGE_READWRITE,
            (PAGE_EXECUTE_WRITECOPY, false) => PAGE_EXECUTE_READWRITE,
            (PAGE_READWRITE, true) => PAGE_WRITECOPY,
            (PAGE_EXECUTE_READWRITE, true) => PAGE_EXECUTE_WRITECOPY,
            (other, _) => other,
        }
}

unsafe fn protect(runs: &[ProtectionRun], cow: bool) -> Result<(), i32> {
    for run in runs {
        let mut previous = 0;
        if unsafe {
            VirtualProtect(
                run.start as _,
                run.length,
                protection(run.protection, cow),
                &mut previous,
            )
        } == 0
        {
            return Err(last_errno());
        }
    }
    Ok(())
}

pub(super) fn make_remappable(
    registry: &mut HashMap<usize, Mapping>,
    start: usize,
    mut mapping: Mapping,
) -> Result<(), i32> {
    let old = mapping.backing.as_ref().ok_or(EIO)?;
    if !old.is_cow() || mapping.base as usize != start {
        return Err(EINVAL);
    }
    // Before its first fork an anonymous snapshot is an ordinary writable
    // pagefile view: every store already belongs to its section. Keeping that
    // section is sufficient when the view must be split; copying all its pages
    // would fault in untouched neighbours for no benefit. Once fork converts
    // the view to WRITECOPY, private dirty pages require the path below.
    if old.is_snapshot() {
        let mut info: MemoryBasicInformation = unsafe { core::mem::zeroed() };
        if unsafe { VirtualQuery(mapping.base, &mut info, size_of::<MemoryBasicInformation>()) }
            != 0
            && info.allocation_base == mapping.base
            && info.type_ == MEM_MAPPED_TYPE
            && matches!(
                info.allocation_protect & 0xff,
                PAGE_READWRITE | PAGE_EXECUTE_READWRITE
            )
        {
            let mut duplicate = ptr::null_mut();
            if unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    old.handle(),
                    GetCurrentProcess(),
                    &mut duplicate,
                    0,
                    0,
                    2, // DUPLICATE_SAME_ACCESS
                )
            } == 0
            {
                return Err(last_errno());
            }
            let replacement = match BackingRef::new(duplicate, unsafe { (*old.0).length }, true) {
                Ok(backing) => backing,
                Err(error) => {
                    unsafe { CloseHandle(duplicate) };
                    return Err(error);
                }
            };
            // A copied-section record handles subsequent fork/split operations.
            // The existing view and its per-page permissions remain untouched.
            mapping.backing = Some(replacement);
            insert_mapping_fragment(registry, start, mapping);
            return Ok(());
        }
    }
    let runs = protection_runs(start, mapping.length, mapping.view_protection)?;
    let section = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_EXECUTE_READWRITE,
            (mapping.length as u64 >> 32) as u32,
            mapping.length as u32,
            ptr::null(),
        )
    };
    if section.is_null() {
        return Err(last_errno());
    }
    let replacement = match BackingRef::new(section, mapping.length, true) {
        Ok(backing) => backing,
        Err(error) => {
            unsafe { CloseHandle(section) };
            return Err(error);
        }
    };
    let stage = unsafe {
        MapViewOfFileEx(
            section,
            FILE_MAP_WRITE,
            0,
            0,
            mapping.length,
            ptr::null_mut(),
        )
    };
    if stage.is_null() {
        return Err(last_errno());
    }
    let stage = Staging(stage);

    // Everything below the freeze boundary uses preallocated state and native
    // memory calls only. Guest writes to surviving neighbours cannot race the
    // snapshot or encounter the temporary placeholder.
    {
        let _frozen = unsafe { kinakaze_runtime::freeze_memory_threads() }.map_err(|_| EIO)?;
        for run in &runs {
            let mut previous = 0;
            let changed = run.protection & 0x100 != 0
                || matches!(run.protection & 0xff, PAGE_NOACCESS | PAGE_EXECUTE);
            if changed
                && unsafe {
                    VirtualProtect(run.start as _, run.length, PAGE_READONLY, &mut previous)
                } == 0
            {
                return Err(last_errno());
            }
            let mut copied = 0;
            let ok = unsafe {
                ReadProcessMemory(
                    GetCurrentProcess(),
                    run.start as _,
                    stage.0.byte_add(run.start - start),
                    run.length,
                    &mut copied,
                )
            } != 0;
            let error = if ok && copied == run.length {
                None
            } else {
                Some(last_errno())
            };
            if changed {
                let mut ignored = 0;
                if unsafe { VirtualProtect(run.start as _, run.length, previous, &mut ignored) }
                    == 0
                {
                    std::process::abort(); // Never resume with changed guest access.
                }
            }
            if let Some(error) = error {
                return Err(error);
            }
        }
        if unsafe {
            UnmapViewOfFile2(
                GetCurrentProcess(),
                MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: mapping.base,
                },
                MEM_PRESERVE_PLACEHOLDER,
            )
        } == 0
        {
            return Err(last_errno());
        }
        let view = unsafe {
            MapViewOfFile3(
                section,
                GetCurrentProcess(),
                mapping.base,
                0,
                mapping.length,
                MEM_REPLACE_PLACEHOLDER,
                PAGE_EXECUTE_READWRITE,
                ptr::null_mut(),
                0,
            )
        };
        let installed = !view.Value.is_null();
        let result = if installed {
            unsafe { protect(&runs, false) }.and_then(|_| {
                if unsafe {
                    FlushInstructionCache(GetCurrentProcess(), mapping.base, mapping.length)
                } != 0
                {
                    Ok(())
                } else {
                    Err(last_errno())
                }
            })
        } else {
            Err(last_errno())
        };
        if let Err(error) = result {
            // Roll back the complete old view and private bytes before resuming
            // any sibling. An unrecoverable native rollback cannot return.
            if installed
                && unsafe { UnmapViewOfFile2(GetCurrentProcess(), view, MEM_PRESERVE_PLACEHOLDER) }
                    == 0
            {
                std::process::abort();
            }
            let restored = unsafe {
                MapViewOfFile3(
                    old.handle(),
                    GetCurrentProcess(),
                    mapping.base,
                    mapping.backing_offset,
                    mapping.length,
                    MEM_REPLACE_PLACEHOLDER,
                    old.maximum_protection(),
                    ptr::null_mut(),
                    0,
                )
            };
            if restored.Value.is_null() {
                std::process::abort();
            }
            let mut copied = 0;
            if unsafe {
                WriteProcessMemory(
                    GetCurrentProcess(),
                    mapping.base,
                    stage.0,
                    mapping.length,
                    &mut copied,
                )
            } == 0
                || copied != mapping.length
                || unsafe { protect(&runs, true) }.is_err()
                || unsafe {
                    FlushInstructionCache(GetCurrentProcess(), mapping.base, mapping.length)
                } == 0
            {
                std::process::abort();
            }
            return Err(error);
        }
    }

    mapping.backing = Some(replacement);
    mapping.backing_offset = 0;
    mapping.view_protection = PAGE_EXECUTE_READWRITE;
    if let Some(origin) = &mut mapping.file_origin {
        origin.representation = file_origin::Representation::CopiedUnclassified;
    }
    // No allocator or registry mutation occurs while siblings are suspended.
    // The caller retains the topology transaction and registry guard throughout.
    insert_mapping_fragment(registry, start, mapping);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_snapshot_reuses_section_but_forked_cow_keeps_private_bytes() {
        let page = memory_geometry().0;
        for cow in [false, true] {
            let mapped = map(
                ptr::null_mut(),
                page * 3,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
            .unwrap();
            let start = mapped as usize;
            let transaction = kinakaze_runtime::begin_fork_mapping_transaction().unwrap();
            let mut registry = mappings().lock().unwrap();
            let mapping = registry.get(&start).unwrap().clone();
            let old = mapping.backing.as_ref().unwrap().clone();
            unsafe { mapped.cast::<u8>().write(0x51) };
            let alias = unsafe {
                MapViewOfFileEx(old.handle(), FILE_MAP_READ, 0, 0, page * 3, ptr::null_mut())
            };
            assert!(!alias.is_null());
            if cow {
                // The same native conversion performed at first fork. The
                // backing retains 0x51 while this view acquires a private byte.
                assert_ne!(
                    unsafe {
                        UnmapViewOfFile2(
                            GetCurrentProcess(),
                            MEMORY_MAPPED_VIEW_ADDRESS { Value: mapped },
                            MEM_PRESERVE_PLACEHOLDER,
                        )
                    },
                    0
                );
                let view = unsafe {
                    MapViewOfFile3(
                        old.handle(),
                        GetCurrentProcess(),
                        mapped,
                        0,
                        page * 3,
                        MEM_REPLACE_PLACEHOLDER,
                        PAGE_EXECUTE_WRITECOPY,
                        ptr::null_mut(),
                        0,
                    )
                };
                assert_eq!(view.Value, mapped);
                unsafe { mapped.cast::<u8>().write(0xb7) };
            }
            make_remappable(&mut registry, start, mapping).unwrap();
            assert!(
                registry
                    .get(&start)
                    .unwrap()
                    .backing
                    .as_ref()
                    .unwrap()
                    .is_remappable()
            );
            assert_eq!(
                unsafe { mapped.cast::<u8>().read() },
                if cow { 0xb7 } else { 0x51 }
            );
            unsafe { mapped.cast::<u8>().write(0xd3) };
            // Fresh conversion must still refer to the exact original section;
            // the COW path must keep the already-forked backing unchanged.
            assert_eq!(
                unsafe { alias.cast::<u8>().read() },
                if cow { 0x51 } else { 0xd3 }
            );
            drop(registry);
            drop(transaction);
            assert_ne!(unsafe { UnmapViewOfFile(alias) }, 0);
            unmap(mapped, page * 3).unwrap();
        }
    }
}
