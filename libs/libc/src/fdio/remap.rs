//! Transactional private anonymous mremap, preserving native page protections.
use super::*;
use windows_sys::Win32::System::Memory::PAGE_EXECUTE;
mod copy;

pub(crate) fn remap(
    old: usize,
    old_length: usize,
    new_length: usize,
    flags: i32,
    destination: usize,
) -> Result<usize, i32> {
    let (page, _) = memory_geometry();
    if old % page != 0
        || old_length == 0
        || new_length == 0
        || flags & !3 != 0
        || flags & 2 != 0 && flags & 1 == 0
    {
        return Err(EINVAL);
    }
    let old_length = page_rounded_length(old_length)?;
    let new_length = page_rounded_length(new_length)?;
    let old_end = old.checked_add(old_length).ok_or(EINVAL)?;
    if flags & 2 != 0
        && (destination % page != 0
            || destination == 0
            || destination
                .checked_add(new_length)
                .is_none_or(|end| destination < old_end && old < end))
    {
        return Err(EINVAL);
    }
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;
    let mapping = mappings()
        .lock()
        .map_err(|_| EIO)?
        .get(&old)
        .cloned()
        .ok_or(EFAULT)?;
    if mapping.length != old_length {
        return Err(EFAULT);
    }
    if flags & 2 == 0 && new_length <= old_length {
        if new_length < old_length {
            unmap((old + new_length) as _, old_length - new_length)?;
        }
        return Ok(old);
    }
    // Shared/file remapping needs retained section resize and alias updates.
    // Copying such a view would silently destroy its sharing contract.
    if mapping.shared
        || mapping.file.is_some()
        || mapping.file_origin.is_some()
        || mapping.tmpfs.is_some()
        || mapping.native_inode.is_some()
        || mapping.verity.is_some()
        || mapping.kind.is_borrowed()
    {
        return Err(kinakaze_vfs::EOPNOTSUPP);
    }
    let runs = protection_runs(old, old_length, mapping.view_protection)?;
    if flags & 2 == 0 && new_length > old_length {
        // Reserve the adjacent pages without replacing any existing VMA.
        let protection = linux_protection(runs.last().ok_or(EFAULT)?.protection)?;
        if map(
            old_end as _,
            new_length - old_length,
            protection,
            0x100022,
            -1,
            0,
        )
        .is_ok()
        {
            return Ok(old);
        }
        if flags & 1 == 0 {
            return Err(ENOMEM);
        }
    }
    let address = if flags & 2 != 0 { destination } else { 0 };
    let target = map(
        address as _,
        new_length,
        3,
        if flags & 2 != 0 { 0x32 } else { 0x22 },
        -1,
        0,
    )? as usize;
    let transfer = (|| {
        for run in &runs {
            let offset = run.start - old;
            if offset >= new_length {
                break;
            }
            let count = run.length.min(new_length - offset);
            let mut native: MemoryBasicInformation = unsafe { core::mem::zeroed() };
            if unsafe {
                VirtualQuery(
                    run.start as _,
                    &mut native,
                    size_of::<MemoryBasicInformation>(),
                )
            } == 0
            {
                return Err(EFAULT);
            }
            // PROT_NONE reservations contain implicit zero pages, which the
            // destination already has. Committed inaccessible pages retain data.
            if native.state == MEM_COMMIT_STATE {
                let mut saved = 0;
                let inaccessible = matches!(run.protection & 0xff, PAGE_NOACCESS | PAGE_EXECUTE);
                if inaccessible
                    && unsafe {
                        windows_sys::Win32::System::Memory::VirtualProtect(
                            run.start as _,
                            count,
                            if run.protection & 0xff == PAGE_EXECUTE {
                                PAGE_EXECUTE_READ
                            } else {
                                PAGE_READONLY
                            },
                            &mut saved,
                        )
                    } == 0
                {
                    return Err(last_errno());
                }
                // The registry transaction pins both private VMAs. This local
                // SIMD copy avoids a ReadProcessMemory kernel transition.
                unsafe {
                    copy::copy(run.start as _, (target + offset) as _, count);
                }
                let mut unused = 0;
                let restored = !inaccessible
                    || unsafe {
                        windows_sys::Win32::System::Memory::VirtualProtect(
                            run.start as _,
                            count,
                            saved,
                            &mut unused,
                        )
                    } != 0;
                if !restored {
                    return Err(EFAULT);
                }
            }
            if unsafe {
                mprotect_native(
                    (target + offset) as _,
                    count,
                    linux_protection(run.protection)?,
                )
            } != 0
            {
                return Err(kinakaze_tls::errno());
            }
        }
        if new_length > old_length {
            let protection = linux_protection(runs.last().ok_or(EFAULT)?.protection)?;
            if unsafe {
                mprotect_native(
                    (target + old_length) as _,
                    new_length - old_length,
                    protection,
                )
            } != 0
            {
                return Err(kinakaze_tls::errno());
            }
        }
        unmap(old as _, old_length)
    })();
    if let Err(error) = transfer {
        let _ = unmap(target as _, new_length);
        return Err(error);
    }
    Ok(target)
}

fn linux_protection(native: u32) -> Result<i32, i32> {
    use windows_sys::Win32::System::Memory::*;
    Ok(match native & 0xff {
        PAGE_NOACCESS | 0 => 0,
        PAGE_READONLY => 1,
        PAGE_READWRITE | PAGE_WRITECOPY => 3,
        PAGE_EXECUTE => 4,
        PAGE_EXECUTE_READ => 5,
        PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY => 7,
        _ => return Err(EINVAL),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_remap_moves_data_grows_zeros_and_restores_protection() {
        let old = map(ptr::null_mut(), 8192, 3, 0x22, -1, 0).unwrap() as usize;
        unsafe {
            (old as *mut u64).write(0x123456789abcdef);
        }
        let target = map(ptr::null_mut(), 16384, 3, 0x22, -1, 0).unwrap() as usize;
        let moved = remap(old, 8192, 16384, 3, target).unwrap();
        assert_eq!(moved, target);
        assert_eq!(unsafe { (moved as *const u64).read() }, 0x123456789abcdef);
        assert_eq!(unsafe { ((moved + 8192) as *const u64).read() }, 0);
        assert_eq!(remap(moved, 16384, 4096, 0, 0), Ok(moved));
        unmap(moved as _, 4096).unwrap();
        assert_eq!(remap(old + 1, 4096, 8192, 1, 0), Err(EINVAL));
    }

    #[test]
    fn mixed_protections_preserve_committed_inaccessible_data() {
        let old = map(ptr::null_mut(), 12288, 3, 0x22, -1, 0).unwrap() as usize;
        for index in 0..3 {
            unsafe {
                ((old + index * 4096) as *mut u64).write(100 + index as u64);
            }
        }
        assert_eq!(unsafe { mprotect_native(old as _, 4096, 1) }, 0);
        assert_eq!(unsafe { mprotect_native((old + 4096) as _, 4096, 0) }, 0);
        let target = map(ptr::null_mut(), 16384, 3, 0x22, -1, 0).unwrap() as usize;
        assert_eq!(remap(old, 12288, 16384, 3, target), Ok(target));
        for (index, protection) in [PAGE_READONLY, PAGE_NOACCESS, PAGE_READWRITE, PAGE_READWRITE]
            .into_iter()
            .enumerate()
        {
            let mut native: MemoryBasicInformation = unsafe { core::mem::zeroed() };
            assert_ne!(
                unsafe {
                    VirtualQuery(
                        (target + index * 4096) as _,
                        &mut native,
                        size_of::<MemoryBasicInformation>(),
                    )
                },
                0
            );
            assert_eq!(native.protect, protection);
        }
        assert_eq!(unsafe { mprotect_native(target as _, 16384, 3) }, 0);
        for index in 0..3 {
            assert_eq!(
                unsafe { ((target + index * 4096) as *const u64).read() },
                100 + index as u64
            );
        }
        assert_eq!(unsafe { ((target + 12288) as *const u64).read() }, 0);
        unmap(target as _, 16384).unwrap();
    }
}
