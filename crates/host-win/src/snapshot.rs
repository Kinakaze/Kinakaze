//! Immutable metadata and transactional transfer of its file lifetime pins.
use crate::{ProcessHandle, owned};
use std::{
    io,
    os::windows::io::{AsRawHandle, OwnedHandle},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{
        DUPLICATE_CLOSE_SOURCE, DUPLICATE_SAME_ACCESS, DuplicateHandle, INVALID_HANDLE_VALUE,
    },
    System::{Memory::*, Threading::GetCurrentProcess},
};

pub struct ReadOnlySection {
    section: OwnedHandle,
    length: usize,
}
impl ReadOnlySection {
    pub fn new(bytes: &[u8]) -> io::Result<Self> {
        if bytes.is_empty() || bytes.len() > u32::MAX as usize {
            return Err(io::Error::other("invalid metadata section length"));
        }
        let section = unsafe {
            owned(CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_READWRITE,
                0,
                bytes.len() as u32,
                ptr::null(),
            ))?
        };
        let view =
            unsafe { MapViewOfFile(section.as_raw_handle(), FILE_MAP_WRITE, 0, 0, bytes.len()) };
        if view.Value.is_null() {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), view.Value.cast(), bytes.len());
            UnmapViewOfFile(view);
        }
        Ok(Self {
            section,
            length: bytes.len(),
        })
    }
    pub fn length(&self) -> usize {
        self.length
    }
    pub fn transfer(&self, transfer: &mut RemoteTransfer) -> io::Result<()> {
        transfer.add(
            self.section.as_raw_handle(),
            SECTION_QUERY | SECTION_MAP_READ,
            false,
        )
    }
}

/// The authenticated sender transfers exclusive ownership of every handle.
/// All file pins survive for as long as names borrowed from the section do.
pub struct ReadOnlySectionView {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    length: usize,
    _owners: Vec<OwnedHandle>,
}
unsafe impl Send for ReadOnlySectionView {}
unsafe impl Sync for ReadOnlySectionView {}
impl ReadOnlySectionView {
    /// # Safety
    /// `handles` are valid exclusively owned handles in this process; the first
    /// is an immutable read-only section and the rest pin its source files.
    pub unsafe fn adopt(handles: &[u64], length: usize, limit: usize) -> io::Result<Self> {
        // Adopt before validating sizes, so malformed replies still release resources.
        let owners = handles
            .iter()
            .map(|&handle| unsafe { owned(handle as _) })
            .collect::<io::Result<Vec<_>>>()?;
        if owners.is_empty() || length == 0 || length > limit {
            return Err(io::Error::other("invalid metadata section length"));
        }
        let view = unsafe { MapViewOfFile(owners[0].as_raw_handle(), FILE_MAP_READ, 0, 0, length) };
        if view.Value.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            view,
            length,
            _owners: owners,
        })
    }
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.view.Value.cast(), self.length) }
    }
    pub fn pin_count(&self) -> usize {
        self._owners.len() - 1
    }

    /// Identity of one transferred source file, queried through its retained
    /// handle. No pathname is reopened and the deny-write/delete pin stays live.
    pub fn pin_path(&self, index: usize) -> io::Result<std::path::PathBuf> {
        let owner = index
            .checked_add(1)
            .and_then(|index| self._owners.get(index))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid source pin index")
            })?;
        crate::file_map::canonical_path(owner.as_raw_handle())
    }
}
impl Drop for ReadOnlySectionView {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
        }
    }
}

/// Roll back remote capabilities unless the complete RPC reply was sent.
pub struct RemoteTransfer {
    process: ProcessHandle,
    handles: Vec<u64>,
}
impl RemoteTransfer {
    pub fn new(process: ProcessHandle) -> Self {
        Self {
            process,
            handles: Vec::new(),
        }
    }
    pub fn add(
        &mut self,
        source: std::os::windows::io::RawHandle,
        access: u32,
        same_access: bool,
    ) -> io::Result<()> {
        let mut remote = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                source,
                self.process.raw(),
                &mut remote,
                access,
                0,
                if same_access {
                    DUPLICATE_SAME_ACCESS
                } else {
                    0
                },
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        self.handles.push(remote as u64);
        Ok(())
    }
    pub fn handles(&self) -> &[u64] {
        &self.handles
    }
    pub fn commit(mut self) {
        self.handles.clear();
    }
}
impl Drop for RemoteTransfer {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            unsafe {
                DuplicateHandle(
                    self.process.raw(),
                    handle as _,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    0,
                    0,
                    DUPLICATE_CLOSE_SOURCE,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::GetHandleInformation;

    #[test]
    fn transfer_rolls_back_until_committed_and_only_grants_read_access() {
        let section = ReadOnlySection::new(b"immutable metadata").unwrap();
        let peer = || ProcessHandle::open(std::process::id()).unwrap();
        let mut transfer = RemoteTransfer::new(peer());
        section.transfer(&mut transfer).unwrap();
        let raw = transfer.handles()[0];
        let mut flags = 0;
        assert_ne!(unsafe { GetHandleInformation(raw as _, &mut flags) }, 0);
        assert_eq!(
            flags & 1,
            0,
            "a catalog must not leak into fork handle inheritance"
        );
        drop(transfer);
        assert_eq!(unsafe { GetHandleInformation(raw as _, &mut flags) }, 0);

        let mut transfer = RemoteTransfer::new(peer());
        section.transfer(&mut transfer).unwrap();
        let handles = transfer.handles().to_vec();
        assert!(
            unsafe { MapViewOfFile(handles[0] as _, FILE_MAP_WRITE, 0, 0, section.length()) }
                .Value
                .is_null()
        );
        transfer.commit();
        let view = unsafe { ReadOnlySectionView::adopt(&handles, section.length(), 1024) }.unwrap();
        drop(section);
        assert_eq!(view.as_slice(), b"immutable metadata");
        assert_eq!(view.pin_count(), 0);
        assert!(view.pin_path(0).is_err());
        assert!(view.pin_path(usize::MAX).is_err());
    }
}
