//! Directory I/O owned by init, with a queue shared by all guest descriptor aliases.
use std::{
    io,
    os::windows::io::{AsRawHandle, OwnedHandle},
    path::Path,
    ptr,
    sync::Arc,
    thread::JoinHandle,
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{IO::*, Memory::*, Threading::*},
};

const CAPACITY: usize = 1024 * 1024;
const MAGIC: u64 = u64::from_le_bytes(*b"CYDIRQ01");
#[repr(C)]
struct Header {
    magic: u64,
    domain: u64,
    id: u64,
    length: usize,
    overflow: bool,
    ended: bool,
}

pub struct DirectoryQueue {
    _section: OwnedHandle,
    mutex: OwnedHandle,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}
// All shared bytes are protected by the same named kernel mutex.
unsafe impl Send for DirectoryQueue {}
unsafe impl Sync for DirectoryQueue {}
impl Drop for DirectoryQueue {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
        }
    }
}
struct Guard<'a>(&'a DirectoryQueue);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0.mutex.as_raw_handle());
        }
    }
}
impl DirectoryQueue {
    pub fn open(domain: u64, id: u64, create: bool) -> io::Result<Self> {
        let name = format!(r"Local\kinakaze.directory-watch.v1.{domain}.{id}");
        let wide = super::wide(std::ffi::OsStr::new(&name))?;
        let mutex_name = super::wide(std::ffi::OsStr::new(&format!("{name}.guard")))?;
        let section = unsafe {
            super::owned(if create {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_READWRITE,
                    0,
                    (size_of::<Header>() + CAPACITY) as u32,
                    wide.as_ptr(),
                )
            } else {
                OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, wide.as_ptr())
            })?
        };
        let mutex = unsafe { super::owned(CreateMutexW(ptr::null(), 0, mutex_name.as_ptr()))? };
        let view = unsafe {
            MapViewOfFile(
                section.as_raw_handle(),
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                size_of::<Header>() + CAPACITY,
            )
        };
        if view.Value.is_null() {
            return Err(io::Error::last_os_error());
        }
        let value = Self {
            _section: section,
            mutex,
            view,
        };
        {
            let _guard = value.lock()?;
            let header = value.view.Value.cast::<Header>();
            if create && unsafe { (*header).magic } == 0 {
                unsafe {
                    header.write(Header {
                        magic: MAGIC,
                        domain,
                        id,
                        length: 0,
                        overflow: false,
                        ended: false,
                    });
                }
            }
            if unsafe { ((*header).magic, (*header).domain, (*header).id) } != (MAGIC, domain, id) {
                return Err(io::Error::other("directory queue identity mismatch"));
            }
        }
        Ok(value)
    }
    fn lock(&self) -> io::Result<Guard<'_>> {
        match unsafe { WaitForSingleObject(self.mutex.as_raw_handle(), INFINITE) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Guard(self)),
            _ => Err(io::Error::last_os_error()),
        }
    }
    fn push(&self, bytes: &[u8], ended: bool) -> io::Result<()> {
        let _guard = self.lock()?;
        let header = self.view.Value.cast::<Header>();
        // Publish the length only after the complete packet was copied. A dead
        // reader/writer cannot expose a partial notification after abandonment.
        unsafe {
            let length = (*header).length;
            if length > CAPACITY {
                return Err(io::Error::other("invalid directory queue"));
            }
            if bytes.is_empty() || bytes.len() + 4 > CAPACITY - length {
                (*header).overflow |= !ended;
            } else {
                let destination = header.add(1).cast::<u8>().add(length);
                ptr::copy_nonoverlapping(
                    (bytes.len() as u32).to_le_bytes().as_ptr(),
                    destination,
                    4,
                );
                ptr::copy_nonoverlapping(bytes.as_ptr(), destination.add(4), bytes.len());
                (*header).length = length + 4 + bytes.len();
            }
            (*header).ended |= ended;
        }
        Ok(())
    }
    /// Observe readiness without consuming notifications owned by another
    /// reader. The same publisher mutex protects both this check and push.
    pub fn pending(&self) -> io::Result<bool> {
        let _guard = self.lock()?;
        let header = unsafe { &*self.view.Value.cast::<Header>() };
        if header.length > CAPACITY {
            return Err(io::Error::other("invalid directory queue"));
        }
        Ok(header.length != 0 || header.overflow || header.ended)
    }

    pub fn drain(&self) -> io::Result<(Vec<Vec<u8>>, bool, bool)> {
        let _guard = self.lock()?;
        unsafe {
            let header = self.view.Value.cast::<Header>();
            if (*header).length > CAPACITY {
                return Err(io::Error::other("invalid directory queue"));
            }
            let mut bytes =
                std::slice::from_raw_parts(header.add(1).cast::<u8>(), (*header).length);
            let mut packets = Vec::new();
            while !bytes.is_empty() {
                if bytes.len() < 4 {
                    return Err(io::Error::other("truncated directory queue"));
                }
                let length = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
                bytes = &bytes[4..];
                if length > bytes.len() {
                    return Err(io::Error::other("truncated directory event"));
                }
                packets.push(bytes[..length].to_vec());
                bytes = &bytes[length..];
            }
            let result = (packets, (*header).overflow, (*header).ended);
            (*header).length = 0;
            (*header).overflow = false;
            Ok(result)
        }
    }
}

pub struct DirectoryWatch {
    cancel: Arc<OwnedHandle>,
    _queue: Arc<DirectoryQueue>,
    thread: Option<JoinHandle<()>>,
}
impl DirectoryWatch {
    pub fn start(domain: u64, id: u64, path: &Path) -> io::Result<Self> {
        let wide = super::wide(path.as_os_str())?;
        let directory = unsafe {
            super::owned(CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            ))?
        };
        let shared_queue = Arc::new(DirectoryQueue::open(domain, id, true)?);
        let queue = shared_queue.clone();
        let cancel =
            Arc::new(unsafe { super::owned(CreateEventW(ptr::null(), 1, 0, ptr::null()))? });
        let stop = cancel.clone();
        let (sender, ready) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("kinakaze-directory-watch".into())
            .spawn(move || {
                let mut first = Some(sender);
                let mut buffer = vec![0u8; 65536];
                loop {
                    let event =
                        match unsafe { super::owned(CreateEventW(ptr::null(), 1, 0, ptr::null())) }
                        {
                            Ok(event) => event,
                            Err(error) => {
                                if let Some(sender) = first.take() {
                                    let _ = sender.send(Err(error));
                                }
                                break;
                            }
                        };
                    let mut operation: OVERLAPPED = unsafe { std::mem::zeroed() };
                    operation.hEvent = event.as_raw_handle();
                    let accepted = unsafe {
                        ReadDirectoryChangesW(
                            directory.as_raw_handle(),
                            buffer.as_mut_ptr().cast(),
                            buffer.len() as u32,
                            0,
                            FILE_NOTIFY_CHANGE_FILE_NAME
                                | FILE_NOTIFY_CHANGE_DIR_NAME
                                | FILE_NOTIFY_CHANGE_ATTRIBUTES
                                | FILE_NOTIFY_CHANGE_SIZE
                                | FILE_NOTIFY_CHANGE_LAST_WRITE
                                | FILE_NOTIFY_CHANGE_CREATION,
                            ptr::null_mut(),
                            &mut operation,
                            None,
                        )
                    };
                    if accepted == 0 {
                        let error = io::Error::last_os_error();
                        if let Some(sender) = first.take() {
                            let _ = sender.send(Err(error));
                        }
                        let _ = queue.push(&[], true);
                        break;
                    }
                    if let Some(sender) = first.take() {
                        let _ = sender.send(Ok(()));
                    }
                    let handles = [event.as_raw_handle(), stop.as_raw_handle()];
                    let result =
                        unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
                    let mut length = 0;
                    if result != WAIT_OBJECT_0 {
                        unsafe {
                            CancelIoEx(directory.as_raw_handle(), &operation);
                            GetOverlappedResult(
                                directory.as_raw_handle(),
                                &operation,
                                &mut length,
                                1,
                            );
                        }
                        break;
                    }
                    if unsafe {
                        GetOverlappedResult(directory.as_raw_handle(), &operation, &mut length, 0)
                    } == 0
                    {
                        let _ = queue.push(&[], true);
                        break;
                    }
                    let _ = queue.push(&buffer[..(length as usize).min(buffer.len())], false);
                }
            })?;
        let value = Self {
            cancel,
            _queue: shared_queue,
            thread: Some(thread),
        };
        ready
            .recv()
            .map_err(|_| io::Error::other("directory watcher stopped during startup"))??;
        Ok(value)
    }
}
impl Drop for DirectoryWatch {
    fn drop(&mut self) {
        unsafe {
            SetEvent(self.cancel.as_raw_handle());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
