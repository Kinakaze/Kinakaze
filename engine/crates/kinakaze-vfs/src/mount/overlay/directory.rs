//! One merged readdir cache/cursor per open file description, shared through
//! dup, fork and exec. Rewind drops the snapshot. No pathname is used as a key.
use crate::{EINTR, EINVAL, EIO, ENOMEM, errno_from_win32};
use std::ptr;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEM_COMMIT, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, PAGE_READWRITE, SEC_RESERVE, UnmapViewOfFile, VirtualAlloc,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, INFINITE, ReleaseMutex, WaitForMultipleObjects,
};

const CAPACITY: usize = 64 * 1024 * 1024;
const MAGIC: u64 = u64::from_le_bytes(*b"CYOVDIR1");
#[repr(C)]
struct Header {
    magic: u64,
    cursor: u64,
    bytes: u32,
    count: u32,
}
const HEADER: usize = std::mem::size_of::<Header>();

#[derive(Clone, Debug)]
pub struct DirectoryRecord {
    pub name: String,
    pub inode: u64,
    pub kind: u8,
}

pub(super) struct Directory {
    pub(super) name: String,
    section: usize,
    view: usize,
    mutex: usize,
}
impl Drop for Directory {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view as _,
            });
            CloseHandle(self.section as _);
            CloseHandle(self.mutex as _);
        }
    }
}
struct Guard(usize);
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0 as _);
        }
    }
}

impl Directory {
    pub(super) fn create() -> Result<Self, i32> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| EIO)?
            .as_nanos();
        let name = format!(
            "{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        Self::open(&name, true)
    }
    pub(super) fn open(name: &str, create: bool) -> Result<Self, i32> {
        if name.is_empty()
            || name.len() > 100
            || !name.bytes().all(|b| b.is_ascii_digit() || b == b'-')
        {
            return Err(EIO);
        }
        let section_name: Vec<u16> = format!("Local\\kinakaze.overlay.directory.v1.{name}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mutex_name: Vec<u16> = format!("Local\\kinakaze.overlay.directory.lock.v1.{name}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let section = unsafe {
            if create {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_READWRITE | SEC_RESERVE,
                    0,
                    CAPACITY as u32,
                    section_name.as_ptr(),
                )
            } else {
                OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, section_name.as_ptr())
            }
        };
        if section.is_null() {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        let view = unsafe { MapViewOfFile(section, FILE_MAP_ALL_ACCESS, 0, 0, CAPACITY) };
        if view.Value.is_null() {
            unsafe {
                CloseHandle(section);
            }
            return Err(ENOMEM);
        }
        if unsafe { VirtualAlloc(view.Value, HEADER, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
            unsafe {
                UnmapViewOfFile(view);
                CloseHandle(section);
            }
            return Err(ENOMEM);
        }
        let mutex = unsafe { CreateMutexW(ptr::null(), 0, mutex_name.as_ptr()) };
        if mutex.is_null() {
            unsafe {
                UnmapViewOfFile(view);
                CloseHandle(section);
            }
            return Err(EIO);
        }
        let directory = Self {
            name: name.into(),
            section: section as usize,
            view: view.Value as usize,
            mutex: mutex as usize,
        };
        let _guard = directory.lock()?;
        let header = directory.header();
        if create {
            *header = Header {
                magic: MAGIC,
                cursor: 0,
                bytes: 0,
                count: 0,
            };
        } else if header.magic != MAGIC || header.bytes as usize > CAPACITY - HEADER {
            return Err(EIO);
        }
        Ok(directory)
    }
    fn lock(&self) -> Result<Guard, i32> {
        let interrupt = crate::interrupt::current();
        if interrupt.is_null() {
            return Err(EIO);
        }
        loop {
            crate::signal::register_waiter();
            if crate::signal::pending() & !crate::signal::blocked_mask() != 0 {
                crate::signal::unregister_waiter();
                return Err(EINTR);
            }
            let handles = [self.mutex as _, interrupt];
            let result = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
            crate::signal::unregister_waiter();
            match result {
                WAIT_OBJECT_0 => return Ok(Guard(self.mutex)),
                WAIT_ABANDONED => {
                    self.header().bytes = 0;
                    return Ok(Guard(self.mutex));
                }
                value if value == WAIT_OBJECT_0 + 1 => {}
                _ => return Err(EIO),
            }
        }
    }
    // All accesses occur under this object's named mutex, including other
    // processes' views. The slice carries no pointer into a guest address space.
    #[allow(clippy::mut_from_ref)]
    fn header(&self) -> &mut Header {
        unsafe { &mut *(self.view as *mut Header) }
    }

    pub(super) fn seek(&self, offset: i64, whence: i32) -> Result<u64, i32> {
        let _guard = self.lock()?;
        let header = self.header();
        let base = match whence {
            0 => 0,
            1 => header.cursor,
            _ => return Err(EINVAL),
        };
        let cursor = if offset >= 0 {
            base.checked_add(offset as u64)
        } else {
            base.checked_sub(offset.unsigned_abs())
        }
        .ok_or(EINVAL)?;
        if cursor > i64::MAX as u64 {
            return Err(EINVAL);
        }
        header.cursor = cursor;
        if cursor == 0 {
            header.bytes = 0;
        }
        Ok(cursor)
    }

    pub(super) fn read(
        &self,
        output: &mut [u8],
        wide: bool,
        fill: impl FnOnce() -> Result<Vec<DirectoryRecord>, i32>,
    ) -> Result<usize, i32> {
        let _guard = self.lock()?;
        let header = self.header();
        if header.bytes == 0 {
            let records = fill()?;
            let mut bytes = Vec::new();
            for record in &records {
                let name = record.name.as_bytes();
                if name.len() > 255 {
                    return Err(crate::ENAMETOOLONG);
                }
                bytes.extend_from_slice(&record.inode.to_le_bytes());
                bytes.push(record.kind);
                bytes.push(name.len() as u8);
                bytes.extend_from_slice(name);
            }
            if bytes.len() > CAPACITY - HEADER {
                return Err(ENOMEM);
            }
            if unsafe {
                VirtualAlloc(
                    self.view as _,
                    HEADER + bytes.len(),
                    MEM_COMMIT,
                    PAGE_READWRITE,
                )
            }
            .is_null()
            {
                return Err(ENOMEM);
            }
            unsafe {
                ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    (self.view + HEADER) as *mut u8,
                    bytes.len(),
                );
            }
            header.count = records.len() as u32;
            header.bytes = bytes.len() as u32;
        }
        let bytes = unsafe {
            std::slice::from_raw_parts((self.view + HEADER) as *const u8, header.bytes as usize)
        };
        let mut at = 0;
        let mut index = 0u64;
        let mut written = 0;
        while at < bytes.len() {
            if bytes.len() - at < 10 {
                return Err(EIO);
            }
            let inode = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
            let kind = bytes[at + 8];
            let length = bytes[at + 9] as usize;
            let name = bytes.get(at + 10..at + 10 + length).ok_or(EIO)?;
            at += 10 + length;
            if index < header.cursor {
                index += 1;
                continue;
            }
            let record_len = (20 + length).next_multiple_of(8);
            if output.len() - written < record_len {
                if written == 0 {
                    return Err(EINVAL);
                }
                break;
            }
            let record = &mut output[written..written + record_len];
            record.fill(0);
            record[..8].copy_from_slice(&inode.to_le_bytes());
            record[8..16].copy_from_slice(&(index + 1).to_le_bytes());
            record[16..18].copy_from_slice(&(record_len as u16).to_le_bytes());
            if wide {
                record[18] = kind;
                record[19..19 + length].copy_from_slice(name);
            } else {
                record[18..18 + length].copy_from_slice(name);
                record[record_len - 1] = kind;
            }
            written += record_len;
            index += 1;
            header.cursor = index;
        }
        Ok(written)
    }
}
