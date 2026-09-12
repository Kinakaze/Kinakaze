//! Fork a retained COW view without copying pages still backed by its section.
//! The caller freezes guest writers and pins the original backing until done.
use core::ffi::c_void;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

#[repr(C)]
#[derive(Clone, Copy)]
struct Page {
    address: *const c_void,
    flags: usize,
}

#[link(name = "psapi")]
unsafe extern "system" {
    fn QueryWorkingSetEx(process: *mut c_void, pages: *mut c_void, bytes: u32) -> i32;
}

#[derive(Default, Debug)]
pub(super) struct CopyStats {
    pub copied: usize,
    pub shared: usize,
    pub calls: usize,
}

/// `source..source+length` is a page-aligned committed COW view of exactly the
/// section mapped by the destination. Writers/topology changes are frozen.
/// A nonresident page or a failed query is conservatively copied: VirtualQuery
/// alone cannot tell a modified COW page from an original section page.
pub(super) unsafe fn copy_private_pages<E>(
    source: usize,
    length: usize,
    copy: impl FnMut(usize, usize) -> Result<(), E>,
) -> Result<CopyStats, E> {
    unsafe { copy_pages(source, length, false, copy) }
}

/// As above, but additionally requires readable pagefile-backed source memory.
/// Fault nonresident pages in before inspecting them, so a cold clean mapping
/// need not be copied in full. File mappings keep the conservative path: reads
/// can fault if an external writer truncates their backing.
pub(super) unsafe fn copy_readable_private_pages<E>(
    source: usize,
    length: usize,
    copy: impl FnMut(usize, usize) -> Result<(), E>,
) -> Result<CopyStats, E> {
    unsafe { copy_pages(source, length, true, copy) }
}

unsafe fn copy_pages<E>(
    source: usize,
    length: usize,
    prefault: bool,
    mut copy: impl FnMut(usize, usize) -> Result<(), E>,
) -> Result<CopyStats, E> {
    const PAGE_SIZE: usize = 4096; // Supported guest/host architecture: x86-64.
    const BATCH: usize = 256;
    const VALID_SHARED: usize = 1 | (1 << 15);
    debug_assert_eq!(source % PAGE_SIZE, 0);
    debug_assert_eq!(length % PAGE_SIZE, 0);
    let mut pages = [Page {
        address: core::ptr::null(),
        flags: 0,
    }; BATCH];
    let mut stats = CopyStats::default();
    let mut offset = 0;
    while offset < length {
        let count = ((length - offset) / PAGE_SIZE).min(BATCH);
        for (index, page) in pages[..count].iter_mut().enumerate() {
            *page = Page {
                address: (source + offset + index * PAGE_SIZE) as _,
                flags: 0,
            };
        }
        let mut queried = unsafe {
            QueryWorkingSetEx(
                GetCurrentProcess(),
                pages.as_mut_ptr().cast(),
                (count * core::mem::size_of::<Page>()) as u32,
            )
        } != 0;
        if queried && prefault && pages[..count].iter().any(|page| page.flags & 1 == 0) {
            for page in &pages[..count] {
                if page.flags & 1 == 0 {
                    unsafe {
                        page.address.cast::<u8>().read_volatile();
                    }
                }
            }
            for page in &mut pages[..count] {
                page.flags = 0;
            }
            queried = unsafe {
                QueryWorkingSetEx(
                    GetCurrentProcess(),
                    pages.as_mut_ptr().cast(),
                    (count * core::mem::size_of::<Page>()) as u32,
                )
            } != 0;
        }
        let mut run = None;
        for index in 0..=count {
            let private =
                index < count && (!queried || pages[index].flags & VALID_SHARED != VALID_SHARED);
            if private {
                run.get_or_insert(index);
            } else {
                if let Some(start) = run.take() {
                    let bytes = (index - start) * PAGE_SIZE;
                    copy(offset + start * PAGE_SIZE, bytes)?;
                    stats.copied += bytes;
                    stats.calls += 1;
                }
                if index < count {
                    stats.shared += PAGE_SIZE;
                }
            }
        }
        offset += count * PAGE_SIZE;
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::Debug::WriteProcessMemory,
            Memory::{
                CreateFileMappingW, FILE_MAP_COPY, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE,
                UnmapViewOfFile,
            },
        },
    };

    #[test]
    fn dirty_pages_are_preserved_and_both_writers_remain_private() {
        unsafe {
            let length = 16 * 4096;
            let section = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                core::ptr::null(),
                PAGE_READWRITE,
                0,
                length as u32,
                core::ptr::null(),
            );
            assert!(!section.is_null());
            let backing = MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, length);
            let parent = MapViewOfFile(section, FILE_MAP_COPY, 0, 0, length);
            let child = MapViewOfFile(section, FILE_MAP_COPY, 0, 0, length);
            assert!(!backing.Value.is_null() && !parent.Value.is_null() && !child.Value.is_null());
            let parent_bytes = parent.Value.cast::<u8>();
            let child_bytes = child.Value.cast::<u8>();
            for offset in (0..length).step_by(4096) {
                backing.Value.cast::<u8>().add(offset).write_volatile(17);
                assert_eq!(parent_bytes.add(offset).read_volatile(), 17);
            }
            parent_bytes.add(4096).write_volatile(31);
            parent_bytes.add(8192).write_volatile(37);
            let stats = copy_private_pages(parent_bytes as usize, length, |offset, bytes| {
                let mut written = 0;
                let ok = WriteProcessMemory(
                    GetCurrentProcess(),
                    child_bytes.add(offset).cast(),
                    parent_bytes.add(offset).cast(),
                    bytes,
                    &mut written,
                );
                assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
                assert_eq!(written, bytes);
                Ok::<_, ()>(())
            })
            .unwrap();
            assert_eq!(stats.copied, 8192);
            assert_eq!(stats.shared, length - 8192);
            assert_eq!(stats.calls, 1);
            assert_eq!(child_bytes.add(4096).read_volatile(), 31);
            parent_bytes.add(4096).write_volatile(43);
            child_bytes.add(12288).write_volatile(47);
            assert_eq!(child_bytes.add(4096).read_volatile(), 31);
            assert_eq!(parent_bytes.add(12288).read_volatile(), 17);
            assert_eq!(backing.Value.cast::<u8>().add(4096).read_volatile(), 17);
            UnmapViewOfFile(child);
            UnmapViewOfFile(parent);
            UnmapViewOfFile(backing);
            CloseHandle(section);
        }
    }
    #[test]
    fn nonresident_clean_and_dirty_pages_are_rechecked_after_read_faults() {
        #[link(name = "psapi")]
        unsafe extern "system" {
            fn EmptyWorkingSet(process: *mut c_void) -> i32;
        }
        unsafe {
            let length = 16 * 4096;
            let section = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                core::ptr::null(),
                PAGE_READWRITE,
                0,
                length as u32,
                core::ptr::null(),
            );
            assert!(!section.is_null());
            let parent = MapViewOfFile(section, FILE_MAP_COPY, 0, 0, length);
            let child = MapViewOfFile(section, FILE_MAP_COPY, 0, 0, length);
            assert!(!parent.Value.is_null() && !child.Value.is_null());
            parent.Value.cast::<u8>().add(4096).write_volatile(71);
            parent.Value.cast::<u8>().add(8192).write_volatile(73);
            assert_ne!(EmptyWorkingSet(GetCurrentProcess()), 0);
            let mut cold = Page {
                address: parent.Value,
                flags: 0,
            };
            assert_ne!(
                QueryWorkingSetEx(
                    GetCurrentProcess(),
                    core::ptr::addr_of_mut!(cold).cast(),
                    core::mem::size_of::<Page>() as u32
                ),
                0
            );
            assert_eq!(cold.flags & 1, 0, "the clean page must start nonresident");
            let stats =
                copy_readable_private_pages(parent.Value as usize, length, |offset, bytes| {
                    let mut written = 0;
                    assert_ne!(
                        WriteProcessMemory(
                            GetCurrentProcess(),
                            child.Value.byte_add(offset),
                            parent.Value.byte_add(offset),
                            bytes,
                            &mut written
                        ),
                        0
                    );
                    assert_eq!(written, bytes);
                    Ok::<_, ()>(())
                })
                .unwrap();
            assert_eq!(stats.copied, 8192);
            assert_eq!(stats.shared, length - 8192);
            assert_eq!(child.Value.cast::<u8>().add(4096).read_volatile(), 71);
            assert_eq!(child.Value.cast::<u8>().add(8192).read_volatile(), 73);
            UnmapViewOfFile(child);
            UnmapViewOfFile(parent);
            CloseHandle(section);
        }
    }
}
