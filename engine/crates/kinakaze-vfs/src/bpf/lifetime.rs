//! Native references survive fork, exec, SCM_RIGHTS and abrupt process death.
//! Init retains the registry, never these last-close tokens.
use super::*;
use crate::fs::object::Object;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, GetLastError};
use windows_sys::Win32::System::Memory::{FILE_MAP_READ, OpenFileMappingW};

const MAGIC: u64 = u64::from_le_bytes(*b"CYBPFFD1");

fn name(id: u32) -> Vec<u16> {
    wide(&format!(
        "Local\\kinakaze.bpf-program.v1.{}.{id}",
        kinakaze_runtime::authority::domain_id()
    ))
}

pub(crate) struct Pin {
    id: u32,
    object: Object,
}
impl Pin {
    pub(crate) fn id(&self) -> u32 {
        self.id
    }

    /// Called with the registry locked, after validating the program record.
    pub(super) fn open(id: u32) -> Result<Self, i32> {
        let object = Object::owned(unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                core::ptr::null(),
                PAGE_READWRITE,
                0,
                4096,
                name(id).as_ptr(),
            )
        })?;
        let view = unsafe { MapViewOfFile(object.raw(), FILE_MAP_ALL_ACCESS, 0, 0, 24) };
        if view.Value.is_null() {
            return Err(EIO);
        }
        let expected = [MAGIC, kinakaze_runtime::authority::domain_id(), id as u64];
        let valid = unsafe {
            let words = &mut *view.Value.cast::<[u64; 3]>();
            if words[0] == 0 {
                *words = expected;
            }
            let valid = *words == expected;
            UnmapViewOfFile(view);
            valid
        };
        if !valid {
            return Err(EIO);
        }
        Ok(Self { id, object })
    }

    pub(super) fn from_fd(fd: i32) -> Result<Self, i32> {
        let object = {
            let table = crate::table().read().map_err(|_| EIO)?;
            let entry = table
                .slots
                .get(usize::try_from(fd).map_err(|_| EBADF)?)
                .and_then(|entry| *entry)
                .ok_or(EBADF)?;
            if entry.kind != FdKind::BpfProgram || entry.raw == 0 {
                return Err(EBADF);
            }
            Object::duplicate(entry.raw as HANDLE)?
        };
        let view = unsafe { MapViewOfFile(object.raw(), FILE_MAP_READ, 0, 0, 24) };
        if view.Value.is_null() {
            return Err(EBADF);
        }
        let words = unsafe { view.Value.cast::<[u64; 3]>().read() };
        unsafe { UnmapViewOfFile(view) };
        if words[0] != MAGIC
            || words[1] != kinakaze_runtime::authority::domain_id()
            || words[2] == 0
            || words[2] > u32::MAX as u64
        {
            return Err(EBADF);
        }
        Ok(Self {
            id: words[2] as u32,
            object,
        })
    }

    pub(super) fn install(self) -> Result<i32, i32> {
        let fd = crate::install(
            self.object.raw() as usize,
            FdKind::BpfProgram,
            FdFlags::CLOSE_ON_EXEC,
        )?;
        self.object.into_raw();
        Ok(fd)
    }
}

pub(super) fn alive(id: u32) -> bool {
    let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, name(id).as_ptr()) };
    if handle.is_null() {
        // Access/resource failures are not proof that the last reference closed.
        return unsafe { GetLastError() } != ERROR_FILE_NOT_FOUND;
    }
    unsafe { CloseHandle(handle) };
    true
}
