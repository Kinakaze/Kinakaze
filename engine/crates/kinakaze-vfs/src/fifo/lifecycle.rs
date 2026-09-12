//! Kernel-owned endpoint lifetime and directory change notification.
//!
//! A token is not a data pipe. Its only purpose is to retain one Linux open file
//! description through native HANDLE duplication/inheritance. DELETE_ON_CLOSE
//! removes its private directory entry on the last close, even on process death.

#[cfg(test)]
use std::ffi::OsStr;
use std::mem::size_of;
#[cfg(test)]
use std::os::windows::ffi::OsStrExt;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_NOTIFY_ENUM_DIR,
    ERROR_OPERATION_ABORTED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};

use crate::{EIO, errno_from_win32};

#[cfg(test)]
pub(super) fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

pub(super) fn last_errno() -> i32 {
    errno_from_win32(unsafe { GetLastError() })
}

/// Host implementation state must not depend on a guest's TMP/TEMP environment.
/// Resolve the actual host user's known folder; never substitute a temp path if
/// the Windows profile lookup fails.
pub(crate) fn storage_root() -> Result<PathBuf, i32> {
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHGetKnownFolderPath(
            folder: *const windows_sys::core::GUID,
            flags: u32,
            token: HANDLE,
            path: *mut *mut u16,
        ) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(value: *const std::ffi::c_void);
    }
    struct FolderPath(*mut u16);
    impl Drop for FolderPath {
        fn drop(&mut self) {
            unsafe { CoTaskMemFree(self.0.cast()) };
        }
    }
    const LOCAL_APP_DATA: windows_sys::core::GUID =
        windows_sys::core::GUID::from_u128(0xf1b32785_6fba_4fcf_9d55_7b8e7f157091);
    let mut folder = FolderPath(ptr::null_mut());
    let result =
        unsafe { SHGetKnownFolderPath(&LOCAL_APP_DATA, 0, ptr::null_mut(), &mut folder.0) };
    if result < 0 {
        return Err(if result as u32 & 0xffff_0000 == 0x8007_0000 {
            errno_from_win32(result as u32 & 0xffff)
        } else {
            EIO
        });
    }
    if folder.0.is_null() {
        return Err(EIO);
    }
    let mut length = 0;
    while unsafe { *folder.0.add(length) } != 0 {
        if length == 32_767 {
            return Err(crate::ENAMETOOLONG);
        }
        length += 1;
    }
    let path = PathBuf::from(std::ffi::OsString::from_wide(unsafe {
        std::slice::from_raw_parts(folder.0, length)
    }));
    if !path.is_absolute() {
        return Err(EIO);
    }
    Ok(path.join("kinakaze").join("fifo-v3"))
}

pub(crate) struct Owned(pub HANDLE);
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}

impl Owned {
    pub(super) fn checked(handle: HANDLE) -> Result<Self, i32> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(last_errno())
        } else {
            Ok(Self(handle))
        }
    }

    pub(crate) fn into_raw(self) -> HANDLE {
        let raw = self.0;
        std::mem::forget(self);
        raw
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub(crate) fn token(path: &Path, inherit: bool) -> Result<Owned, i32> {
    let path = crate::path::wide_path(path)?;
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: i32::from(inherit),
    };
    Owned::checked(unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &security,
            CREATE_NEW,
            FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE,
            ptr::null_mut(),
        )
    })
}

/// A private OVERLAPPED request; no request buffer or handle is inherited.
pub(crate) struct DirectoryWatch {
    directory: Owned,
    event: Owned,
    overlapped: Box<OVERLAPPED>,
    buffer: Box<[u64; 512]>,
    armed: bool,
}

unsafe impl Send for DirectoryWatch {}

impl DirectoryWatch {
    pub(crate) fn new(path: &Path) -> Result<Self, i32> {
        let path = crate::path::wide_path(path)?;
        let directory = Owned::checked(unsafe {
            CreateFileW(
                path.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        })?;
        let event = Owned::checked(unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) })?;
        let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { std::mem::zeroed() });
        overlapped.hEvent = event.0;
        let mut watch = Self {
            directory,
            event,
            overlapped,
            buffer: Box::new([0; 512]),
            armed: false,
        };
        watch.rearm()?;
        Ok(watch)
    }

    pub(crate) fn event(&self) -> HANDLE {
        self.event.0
    }

    #[cfg(test)]
    pub(super) fn cancel_for_test(&self) {
        assert_ne!(
            unsafe { CancelIoEx(self.directory.0, &*self.overlapped) },
            0
        );
    }

    /// Called before examining liveness under the FIFO mutex. Overflow also
    /// counts as a change: the caller always enumerates the authoritative names.
    pub(crate) fn rearm(&mut self) -> Result<bool, i32> {
        self.rearm_notifying(|| Ok(()))
    }

    /// A shared observer must publish the consumed notification before resetting
    /// its event. Otherwise its death before reconciliation can strand waiters.
    pub(super) fn rearm_notifying(
        &mut self,
        before_reset: impl FnOnce() -> Result<(), i32>,
    ) -> Result<bool, i32> {
        let mut changed = false;
        if self.armed {
            if unsafe { WaitForSingleObject(self.event.0, 0) } != WAIT_OBJECT_0 {
                return Ok(false);
            }
            let mut transferred = 0;
            let ok = unsafe {
                GetOverlappedResult(self.directory.0, &*self.overlapped, &mut transferred, 0)
            };
            if ok == 0 {
                let error = unsafe { GetLastError() };
                if error == ERROR_IO_INCOMPLETE {
                    return Err(EIO);
                }
                // Windows cancels directory notification submitted by a thread
                // that subsequently exits. This is an internal subscription,
                // not the current caller's I/O: rearm then fully rescan names,
                // exactly as after notification-buffer overflow.
                if !matches!(error, ERROR_NOTIFY_ENUM_DIR | ERROR_OPERATION_ABORTED) {
                    return Err(errno_from_win32(error));
                }
            }
            before_reset()?;
            self.armed = false;
            changed = true;
        }
        unsafe { ResetEvent(self.event.0) };
        let ok = unsafe {
            ReadDirectoryChangesW(
                self.directory.0,
                self.buffer.as_mut_ptr().cast(),
                size_of::<[u64; 512]>() as u32,
                0,
                FILE_NOTIFY_CHANGE_FILE_NAME,
                ptr::null_mut(),
                &mut *self.overlapped,
                None,
            )
        };
        if ok == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(last_errno());
        }
        self.armed = true;
        Ok(changed)
    }
}

impl Drop for DirectoryWatch {
    fn drop(&mut self) {
        if self.armed {
            // Retire before freeing the pinned OVERLAPPED and notification buffer.
            unsafe {
                CancelIoEx(self.directory.0, &*self.overlapped);
                let mut transferred = 0;
                GetOverlappedResult(self.directory.0, &*self.overlapped, &mut transferred, 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW, GetCurrentProcess, PROCESS_INFORMATION,
        STARTUPINFOW, TerminateProcess,
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "kinakaze-fifo-lifetime-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn names(&self) -> Vec<String> {
            std::fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn token_last_duplicate_close_notifies_without_probing_delete_pending() {
        let root = Fixture::new();
        let mut watch = DirectoryWatch::new(&root.0).unwrap();
        let first = token(&root.0.join("fd-1"), false).unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(watch.event(), 5000) },
            WAIT_OBJECT_0
        );
        watch.rearm().unwrap();
        assert_eq!(root.names(), vec!["fd-1"]);
        let mut duplicate = ptr::null_mut();
        assert_ne!(
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    first.0,
                    GetCurrentProcess(),
                    &mut duplicate,
                    0,
                    1,
                    DUPLICATE_SAME_ACCESS,
                )
            },
            0
        );
        let duplicate = Owned::checked(duplicate).unwrap();
        drop(first);
        // Enumeration remains authoritative even if opening a pending-delete
        // token by pathname would fail. One duplicated File Object is still live.
        assert_eq!(root.names(), vec!["fd-1"]);
        drop(duplicate);
        assert_eq!(
            unsafe { WaitForSingleObject(watch.event(), 5000) },
            WAIT_OBJECT_0
        );
        assert!(root.names().is_empty());
    }

    #[test]
    fn inherited_token_survives_unresumed_child_then_notifies_on_termination() {
        let root = Fixture::new();
        let mut watch = DirectoryWatch::new(&root.0).unwrap();
        let endpoint = token(&root.0.join("fd-2"), true).unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(watch.event(), 5000) },
            WAIT_OBJECT_0
        );
        watch.rearm().unwrap();
        let exe = std::env::current_exe().unwrap();
        let application = wide(exe.as_os_str());
        let mut command = format!("\"{}\" --list", exe.display())
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup.cb = size_of::<STARTUPINFOW>() as u32;
        let mut child: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        assert_ne!(
            unsafe {
                CreateProcessW(
                    application.as_ptr(),
                    command.as_mut_ptr(),
                    ptr::null(),
                    ptr::null(),
                    1,
                    CREATE_SUSPENDED | CREATE_NO_WINDOW,
                    ptr::null(),
                    ptr::null(),
                    &startup,
                    &mut child,
                )
            },
            0
        );
        let process = Owned::checked(child.hProcess).unwrap();
        let _thread = Owned::checked(child.hThread).unwrap();
        drop(endpoint);
        // No child-side restore or userspace refcount has run at this point.
        assert_eq!(root.names(), vec!["fd-2"]);
        assert_ne!(unsafe { TerminateProcess(process.0, 99) }, 0);
        assert_eq!(
            unsafe { WaitForSingleObject(process.0, 5000) },
            WAIT_OBJECT_0
        );
        assert_eq!(
            unsafe { WaitForSingleObject(watch.event(), 5000) },
            WAIT_OBJECT_0
        );
        assert!(root.names().is_empty());
    }
}
