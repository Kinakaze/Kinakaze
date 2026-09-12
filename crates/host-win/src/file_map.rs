//! Read-only file views. The owning file denies writes and deletion while mapped.
use std::{
    fs::{File, OpenOptions},
    io,
    ops::Deref,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
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

impl ReadOnlyFile {
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
        assert_eq!(view.as_slice(), b"native image");
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::remove_file(&path).is_err());
        drop(view);
        std::fs::remove_file(path).unwrap();
    }
}
