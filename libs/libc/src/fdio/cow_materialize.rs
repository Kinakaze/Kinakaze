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
