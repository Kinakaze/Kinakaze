//! Bounded file-cache prefetch without copying file contents or moving the OFD.
use super::object::Object;
use crate::{EBADF, EINVAL, FdEntry, FdFlags, FdKind, errno_from_win32};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, MapViewOfFile, PAGE_READONLY, PrefetchVirtualMemory,
    UnmapViewOfFile, WIN32_MEMORY_RANGE_ENTRY,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

fn validate(entry: FdEntry) -> Result<(), i32> {
    if entry.flags.contains(FdFlags::PATH_ONLY)
        || (entry.flags.contains(FdFlags::WRITE_ACCESS)
            && !entry.flags.contains(FdFlags::READ_ACCESS))
    {
        return Err(EBADF);
    }
    if !matches!(entry.kind, FdKind::File | FdKind::TmpfsFile) {
        return Err(EINVAL);
    }
    Ok(())
}

pub fn readahead(fd: i32, offset: i64, count: usize) -> Result<(), i32> {
    let entry = crate::get(fd)?;
    validate(entry)?;
    if offset < 0 {
        return Err(EINVAL);
    }
    // tmpfs already owns resident storage. Still validate even for zero length.
    if count == 0 || entry.kind == FdKind::TmpfsFile {
        return Ok(());
    }
    let _deferred = crate::signal::defer_delivery();
    let (entry, _pin) = crate::pin_native_fd(fd, validate)?;
    let length = super::verity::authoritative_size(entry.raw as _)?;
    let offset = offset as u64;
    if offset >= length {
        return Ok(());
    }
    // Readahead is a hint, not a residency guarantee. Bound each request so an
    // untrusted SIZE_MAX hint cannot evict the desktop's entire working set.
    const MAX_PREFETCH: usize = 8 * 1024 * 1024;
    const GRANULARITY: u64 = 65536; // native x86_64 mapping allocation boundary
    let count = count.min(MAX_PREFETCH).min((length - offset) as usize);
    let aligned = offset & !(GRANULARITY - 1);
    let prefix = (offset - aligned) as usize;
    let section = Object::owned(unsafe {
        CreateFileMappingW(
            entry.raw as _,
            std::ptr::null(),
            PAGE_READONLY,
            0,
            0,
            std::ptr::null(),
        )
    })?;
    let view = unsafe {
        MapViewOfFile(
            section.raw(),
            FILE_MAP_READ,
            (aligned >> 32) as u32,
            aligned as u32,
            prefix + count,
        )
    };
    if view.Value.is_null() {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let ranges = WIN32_MEMORY_RANGE_ENTRY {
        VirtualAddress: unsafe { view.Value.cast::<u8>().add(prefix).cast() },
        NumberOfBytes: count,
    };
    // Windows batches disk requests into its physical cache; the temporary
    // mapping and section are retired before return, with no retained buffers.
    let ok = unsafe { PrefetchVirtualMemory(GetCurrentProcess(), 1, &ranges, 0) };
    let error = (ok == 0).then(|| errno_from_win32(unsafe { GetLastError() }));
    unsafe { UnmapViewOfFile(view) };
    error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::windows::io::IntoRawHandle;

    #[test]
    fn prefetch_preserves_contents_position_and_releases_mapping() {
        let path =
            std::env::temp_dir().join(format!("kinakaze-readahead-{}.tmp", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let bytes: Vec<u8> = (0..180_003).map(|n| (n % 251) as u8).collect();
        file.write_all(&bytes).unwrap();
        file.seek(SeekFrom::Start(37)).unwrap();
        let fd = crate::install(
            file.try_clone().unwrap().into_raw_handle() as usize,
            FdKind::File,
            FdFlags::READ_ACCESS
                .union(FdFlags::WRITE_ACCESS)
                .union(FdFlags::SEEKABLE),
        )
        .unwrap();
        for (offset, length) in [
            (0, 0),
            (3, 65539),
            (65539, 100000),
            (180000, usize::MAX),
            (180003, 1),
            (i64::MAX, usize::MAX),
        ] {
            assert_eq!(readahead(fd, offset, length), Ok(()));
            assert_eq!(file.stream_position().unwrap(), 37);
        }
        assert_eq!(readahead(fd, -1, 1), Err(EINVAL));
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut actual = Vec::new();
        file.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes);
        // An accidentally retained mapped section would prevent truncation.
        file.set_len(0).unwrap();
        assert_eq!(readahead(fd, 0, 1), Ok(()));
        crate::close(fd).unwrap();
        drop(file);
        std::fs::remove_file(path).unwrap();
        assert_eq!(readahead(fd, 0, 1), Err(EBADF));
    }

    #[test]
    fn rejects_nonreadable_and_nonfile_descriptors() {
        assert_eq!(readahead(-1, 0, 0), Err(EBADF));
        let (left, right) = crate::unix::socketpair(crate::socket::SOCK_STREAM).unwrap();
        assert_eq!(readahead(left, 0, 0), Err(EINVAL));
        crate::close(left).unwrap();
        crate::close(right).unwrap();
        let path = std::env::temp_dir().join(format!(
            "kinakaze-readahead-write-{}.tmp",
            std::process::id()
        ));
        let file = std::fs::File::create(&path).unwrap();
        let fd = crate::install(
            file.into_raw_handle() as usize,
            FdKind::File,
            FdFlags::WRITE_ACCESS.union(FdFlags::SEEKABLE),
        )
        .unwrap();
        assert_eq!(readahead(fd, 0, 0), Err(EBADF));
        crate::close(fd).unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
