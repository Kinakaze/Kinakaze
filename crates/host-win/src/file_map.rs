//! Read-only file views. The owning file denies writes and deletion while mapped.
use std::{
    fs::{File, OpenOptions},
    io,
    ops::Deref,
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsHandle, AsRawHandle},
    },
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Storage::FileSystem::FILE_SHARE_READ,
    System::Memory::{
        CreateFileMappingW, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        PAGE_READONLY, UnmapViewOfFile,
    },
};

pub struct ReadOnlyFile {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    length: usize,
    _file: File,
}

// SAFETY: The view is immutable, its file denies writes/deletion, and borrowed
// slices cannot outlive the owner. Windows file views have no thread affinity.
unsafe impl Send for ReadOnlyFile {}
unsafe impl Sync for ReadOnlyFile {}

impl ReadOnlyFile {
    pub fn load_library(&self) -> io::Result<crate::Library> {
        crate::Library::open_pinned(self._file.as_handle())
    }

    /// Query the pinned file's identity without opening its pathname again.
    pub fn canonical_path(&self) -> io::Result<PathBuf> {
        canonical_path(self._file.as_raw_handle())
    }
    pub fn transfer_pin(&self, transfer: &mut crate::RemoteTransfer) -> io::Result<()> {
        transfer.add(self._file.as_raw_handle(), 0, true)
    }
    pub fn open(path: &Path, limit: usize) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(path)?;
        let length = usize::try_from(file.metadata()?.len())
            .map_err(|_| io::Error::other("file too large"))?;
        if length == 0 || length > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file size outside mapping limit",
            ));
        }
        // SAFETY: File is live and read-only; no inheritable mapping handle.
        let mapping = unsafe {
            crate::owned(CreateFileMappingW(
                file.as_raw_handle(),
                std::ptr::null(),
                PAGE_READONLY,
                0,
                0,
                std::ptr::null(),
            ))?
        };
        let view = unsafe { MapViewOfFile(mapping.as_raw_handle(), FILE_MAP_READ, 0, 0, length) };
        if view.Value.is_null() {
            return Err(io::Error::last_os_error());
        }
        // The view retains the section; keep the file open to retain its deny-write lock.
        Ok(Self {
            view,
            length,
            _file: file,
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: The view owns this extent and the file cannot change while held.
        unsafe { std::slice::from_raw_parts(self.view.Value.cast(), self.length) }
    }
}

pub(crate) fn canonical_path(handle: std::os::windows::io::RawHandle) -> io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let mut path = vec![0u16; 512];
    loop {
        let length =
            unsafe { GetFinalPathNameByHandleW(handle, path.as_mut_ptr(), path.len() as u32, 0) }
                as usize;
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        if length < path.len() {
            path.truncate(length);
            return Ok(std::ffi::OsString::from_wide(&path).into());
        }
        let capacity = length
            .checked_add(1)
            .filter(|&length| length <= 32768)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "native file path too long")
            })?;
        path.resize(capacity, 0);
    }
}

impl Deref for ReadOnlyFile {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Drop for ReadOnlyFile {
    fn drop(&mut self) {
        // SAFETY: This owner releases exactly one mapping before closing its file.
        unsafe {
            UnmapViewOfFile(self.view);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mapped_bytes_are_stable_until_the_owner_is_released() {
        let path = std::env::temp_dir().join(format!("kinakaze-file-view-{}", std::process::id()));
        std::fs::write(&path, b"native image").unwrap();
        assert!(ReadOnlyFile::open(&path, 2).is_err());
        let view = ReadOnlyFile::open(&path, 1024).unwrap();
        assert_eq!(view.canonical_path().unwrap(), path.canonicalize().unwrap());
        assert_eq!(view.as_slice(), b"native image");
        let view = std::sync::Arc::new(view);
        let retained = std::sync::Arc::clone(&view);
        std::thread::spawn(move || assert_eq!(retained.as_slice(), b"native image"))
            .join()
            .unwrap();
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::remove_file(&path).is_err());
        drop(view);
        std::fs::remove_file(path).unwrap();
    }
}
