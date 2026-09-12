//! Operation-local inode pins. Descriptor lookup, validation and native
//! duplication share one table guard; no I/O or guest callback holds that guard.

use crate::{EBADF, EIO, EOPNOTSUPP, FdEntry, FdKind, errno_from_win32};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, GetLastError};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

// Caller holds the descriptor-table read guard until this returns.
fn duplicate(mut entry: FdEntry) -> Result<(FdEntry, NativePin), i32> {
    if !matches!(entry.kind, FdKind::File | FdKind::Directory) {
        return Err(EOPNOTSUPP);
    }
    let mut handle = std::ptr::null_mut();
    let process = unsafe { GetCurrentProcess() };
    if unsafe {
        DuplicateHandle(
            process,
            entry.raw as _,
            process,
            &mut handle,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle.cast()) };
    let overlay = crate::mount::overlay::reference(entry)?;
    let native = crate::mount::native::reference(entry)?;
    entry.raw = handle.as_raw_handle() as usize;
    Ok((
        entry,
        NativePin {
            handle,
            overlay,
            _native: native,
        },
    ))
}

/// Inode and mount policy captured atomically before close/reuse can intervene.
pub struct NativePin {
    handle: OwnedHandle,
    overlay: Option<crate::mount::overlay::Description>,
    _native: Option<crate::mount::native::Description>,
}
impl AsRawHandle for NativePin {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.as_raw_handle()
    }
}
impl NativePin {
    /// # Safety
    /// `buffer` must contain `len` writable bytes; `entry` belongs to this pin.
    pub unsafe fn read_once(
        &self,
        entry: FdEntry,
        buffer: *mut u8,
        len: usize,
    ) -> Result<usize, i32> {
        use crate::FdFlags;
        if entry.flags.contains(FdFlags::PATH_ONLY)
            || (entry.flags.contains(FdFlags::WRITE_ACCESS)
                && !entry.flags.contains(FdFlags::READ_ACCESS))
        {
            return Err(EBADF);
        }
        let noatime = entry.flags.contains(FdFlags::NOATIME);
        if entry.kind == FdKind::File && (self.overlay.is_some() || noatime) {
            let bytes = if len == 0 {
                &mut []
            } else {
                unsafe { std::slice::from_raw_parts_mut(buffer, len) }
            };
            let count = crate::fs::verity::verified_read_preserving_atime(
                self.as_raw_handle(),
                entry.offset,
                bytes,
            )?
            .ok_or(EIO)?;
            if !noatime && len != 0 {
                if let Some(overlay) = &self.overlay {
                    overlay.accessed();
                }
            }
            return Ok(count);
        }
        unsafe { crate::platform_read_pinned_once(entry, buffer, len) }
    }
}

/// Atomically validates and pins a filesystem descriptor. The returned entry
/// refers to the owned duplicate but retains the original generation/offset.
/// `validate` must not perform I/O, call guest code or reenter the fd table.
/// Drop the pin before dispatching a signal handler or replacing the image.
pub fn pin_native_fd(
    fd: i32,
    validate: impl FnOnce(FdEntry) -> Result<(), i32>,
) -> Result<(FdEntry, NativePin), i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(usize::try_from(fd).map_err(|_| EBADF)?)
        .and_then(|entry| *entry)
        .ok_or(EBADF)?;
    validate(entry)?;
    duplicate(entry)
}

pub(crate) fn read_entry(fd: i32) -> Result<(FdEntry, Option<NativePin>), i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(usize::try_from(fd).map_err(|_| EBADF)?)
        .and_then(|entry| *entry)
        .ok_or(EBADF)?;
    if entry.kind == FdKind::File {
        let (entry, pin) = duplicate(entry)?;
        Ok((entry, Some(pin)))
    } else {
        Ok((entry, None))
    }
}
