//! On-demand immutable file bytes; no ELF parsing or symbol state lives in init.
//! A kernel read lease validates reuse by inode, including in-place writes.
mod hash;
mod read;
use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    fs::File,
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::null,
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{IO::*, Ioctl::*, Memory::*, Threading::*},
};

const MIN_SIZE: usize = 8 * 1024 * 1024;
const MAX_SIZE: usize = 64 * 1024 * 1024;
const BUDGET: usize = 256 * 1024 * 1024;

unsafe fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

struct Request {
    input: REQUEST_OPLOCK_INPUT_BUFFER,
    output: REQUEST_OPLOCK_OUTPUT_BUFFER,
    overlapped: OVERLAPPED,
}
struct Lease {
    file: File,
    event: OwnedHandle,
    request: Box<UnsafeCell<Request>>,
    pending: bool,
}
// Only the I/O manager writes the output/request, until cancellation is drained.
// Cache's mutex serializes all access; validity reads the kernel event only.
unsafe impl Send for Lease {}
impl Lease {
    fn new(file: File) -> io::Result<Self> {
        let event = unsafe { owned(CreateEventW(null(), 1, 0, null()))? };
        let mut lease = Self {
            file,
            event,
            request: Box::new(UnsafeCell::new(Request {
                input: REQUEST_OPLOCK_INPUT_BUFFER {
                    StructureVersion: REQUEST_OPLOCK_CURRENT_VERSION as u16,
                    StructureLength: size_of::<REQUEST_OPLOCK_INPUT_BUFFER>() as u16,
                    RequestedOplockLevel: OPLOCK_LEVEL_CACHE_READ,
                    Flags: REQUEST_OPLOCK_INPUT_FLAG_REQUEST,
                },
                output: unsafe { std::mem::zeroed() },
                overlapped: unsafe { std::mem::zeroed() },
            })),
            pending: false,
        };
        let request = lease.request.get();
        unsafe { (*request).overlapped.hEvent = lease.event.as_raw_handle() };
        let result = unsafe {
            DeviceIoControl(
                lease.file.as_raw_handle(),
                FSCTL_REQUEST_OPLOCK,
                (&raw const (*request).input).cast(),
                size_of::<REQUEST_OPLOCK_INPUT_BUFFER>() as u32,
                (&raw mut (*request).output).cast(),
                size_of::<REQUEST_OPLOCK_OUTPUT_BUFFER>() as u32,
                std::ptr::null_mut(),
                &raw mut (*request).overlapped,
            )
        };
        if result == 0 && unsafe { GetLastError() } == ERROR_IO_PENDING {
            lease.pending = true;
            Ok(lease)
        } else {
            Err(io::Error::other("read lease not granted"))
        }
    }
    fn valid(&self) -> bool {
        self.pending
            && unsafe { WaitForSingleObject(self.event.as_raw_handle(), 0) } == WAIT_TIMEOUT
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.pending {
            let request = self.request.get();
            let mut transferred = 0;
            unsafe {
                CancelIoEx(self.file.as_raw_handle(), &raw const (*request).overlapped);
                GetOverlappedResult(
                    self.file.as_raw_handle(),
                    &raw const (*request).overlapped,
                    &mut transferred,
                    1,
                );
                if (*request).overlapped.Internal == 0x103 {
                    std::process::abort();
                }
            }
        }
    }
}

#[derive(PartialEq, Eq)]
struct Key {
    volume: u64,
    file: [u8; 16],
    length: usize,
}
struct Entry {
    key: Key,
    lease: Lease,
    section: OwnedHandle,
    hash: [u8; 32],
    // Retain read-only residency. Unmapping 30 MiB on the miss path adds several
    // milliseconds; this view shares the already-budgeted backing pages.
    _view: View,
}
struct View(MEMORY_MAPPED_VIEW_ADDRESS);
impl Drop for View {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.0);
        }
    }
}
#[derive(Default)]
struct Images {
    entries: VecDeque<Entry>,
    bytes: usize,
}

struct Command {
    pid: u32,
    birth: u64,
    source: u64,
    length: u64,
    reply: std::sync::mpsc::SyncSender<Option<(u64, [u8; 32])>>,
}
struct Worker {
    sender: std::sync::mpsc::SyncSender<Command>,
    thread: std::thread::JoinHandle<()>,
}
#[derive(Default)]
pub struct Cache {
    worker: Option<Worker>,
}
impl Cache {
    pub fn get(
        &mut self,
        pid: u32,
        birth: u64,
        source: u64,
        length: u64,
    ) -> Option<(u64, [u8; 32])> {
        // Windows cancels outstanding I/O when its issuing thread exits. A
        // connection thread lasts only one guest process, so leases must be
        // issued and retired by this lazily started, session-owned thread.
        if self.worker.is_none() {
            let (sender, receiver) = std::sync::mpsc::sync_channel::<Command>(0);
            let thread = std::thread::Builder::new()
                .name("image-cache".into())
                .spawn(move || {
                    let mut images = Images::default();
                    while let Ok(command) = receiver.recv() {
                        let result =
                            images.get(command.pid, command.birth, command.source, command.length);
                        let _ = command.reply.send(result);
                    }
                })
                .ok()?;
            self.worker = Some(Worker { sender, thread });
        }
        let (reply, receiver) = std::sync::mpsc::sync_channel(1);
        self.worker
            .as_ref()?
            .sender
            .send(Command {
                pid,
                birth,
                source,
                length,
                reply,
            })
            .ok()?;
        receiver.recv().ok().flatten()
    }
}
impl Drop for Cache {
    fn drop(&mut self) {
        if let Some(Worker { sender, thread }) = self.worker.take() {
            drop(sender);
            let _ = thread.join();
        }
    }
}

impl Images {
    /// Duplicate only from the authenticated, birth-checked pipe peer. Cache
    /// failures are optional misses; the worker retains its usual verified I/O.
    pub fn get(
        &mut self,
        pid: u32,
        birth: u64,
        source: u64,
        length: u64,
    ) -> Option<(u64, [u8; 32])> {
        let result = self.try_get(pid, birth, source, usize::try_from(length).ok()?);
        result.ok()
    }
    fn try_get(
        &mut self,
        pid: u32,
        birth: u64,
        source: u64,
        length: usize,
    ) -> io::Result<(u64, [u8; 32])> {
        if !(MIN_SIZE..=MAX_SIZE).contains(&length) || source == 0 || source > isize::MAX as u64 {
            return Err(io::Error::other("cache request outside limits"));
        }
        let process = unsafe {
            owned(OpenProcess(
                PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            ))?
        };
        let mut times: [FILETIME; 4] = unsafe { std::mem::zeroed() };
        let raw_times = times.as_mut_ptr();
        if unsafe {
            GetProcessTimes(
                process.as_raw_handle(),
                raw_times,
                raw_times.add(1),
                raw_times.add(2),
                raw_times.add(3),
            )
        } == 0
            || (u64::from(times[0].dwHighDateTime) << 32 | u64::from(times[0].dwLowDateTime))
                != birth
        {
            return Err(io::Error::other("cache peer identity mismatch"));
        }
        let mut duplicate = std::ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                process.as_raw_handle(),
                source as HANDLE,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let file = File::from(unsafe { owned(duplicate)? });
        let key = file_key(&file, length)?;
        self.entries.retain(|entry| entry.lease.valid());
        self.bytes = self.entries.iter().map(|entry| entry.key.length).sum();
        let index = self.entries.iter().position(|entry| entry.key == key);
        let entry = match index {
            Some(index) => self.entries.remove(index).unwrap(),
            None => {
                let lease = Lease::new(file)?;
                let (section, hash, view) = snapshot(&lease, length)?;
                if !lease.valid() {
                    return Err(io::Error::other("image changed during snapshot"));
                }
                while self.bytes + length > BUDGET {
                    let old = self.entries.pop_front().unwrap();
                    self.bytes -= old.key.length;
                }
                self.bytes += length;
                Entry {
                    key,
                    lease,
                    section,
                    hash,
                    _view: view,
                }
            }
        };
        // No writable capability leaves init. FILE_MAP_COPY needs MAP_READ,
        // and the loader's private executable views additionally need EXECUTE.
        let mut remote = std::ptr::null_mut();
        let result = if entry.lease.valid()
            && unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    entry.section.as_raw_handle(),
                    process.as_raw_handle(),
                    &mut remote,
                    SECTION_QUERY | SECTION_MAP_READ | SECTION_MAP_EXECUTE,
                    0,
                    0,
                )
            } != 0
        {
            Ok((remote as u64, entry.hash))
        } else {
            Err(io::Error::other("image cache unavailable"))
        };
        self.entries.push_back(entry);
        result
    }
}

fn file_key(file: &File, length: usize) -> io::Result<Key> {
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryObject(
            handle: HANDLE,
            class: u32,
            info: *mut std::ffi::c_void,
            length: u32,
            returned: *mut u32,
        ) -> i32;
    }
    let mut basic = [0u32; 14];
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK
        || unsafe {
            NtQueryObject(
                file.as_raw_handle(),
                0,
                basic.as_mut_ptr().cast(),
                size_of::<[u32; 14]>() as u32,
                std::ptr::null_mut(),
            )
        } < 0
        || basic[1] & FILE_READ_DATA == 0
        || basic[1] & (FILE_WRITE_DATA | FILE_APPEND_DATA) != 0
    {
        return Err(io::Error::other("cache requires a readable disk file"));
    }
    let mut id: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    let mut standard: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&raw mut id).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
        || unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileStandardInfo,
                (&raw mut standard).cast(),
                size_of::<FILE_STANDARD_INFO>() as u32,
            )
        } == 0
        || standard.Directory
        || standard.EndOfFile != length as i64
    {
        return Err(io::Error::other("image size or identity changed"));
    }
    Ok(Key {
        volume: id.VolumeSerialNumber,
        file: id.FileId.Identifier,
        length,
    })
}

fn snapshot(lease: &Lease, length: usize) -> io::Result<(OwnedHandle, [u8; 32], View)> {
    let reading = kinakaze_v2_host_win::StartupSpan::begin("image-cache-read");
    let section = unsafe {
        owned(CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            null(),
            PAGE_EXECUTE_READWRITE,
            0,
            length as u32,
            null(),
        ))?
    };
    let view = unsafe { MapViewOfFile(section.as_raw_handle(), FILE_MAP_WRITE, 0, 0, length) };
    if view.Value.is_null() {
        return Err(io::Error::last_os_error());
    }
    let view = View(view);
    let bytes = unsafe { std::slice::from_raw_parts_mut(view.0.Value.cast::<u8>(), length) };
    let count = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(4);
    let chunk = length.div_ceil(count).div_ceil(4096) * 4096;
    std::thread::scope(|scope| {
        let mut threads = Vec::new();
        for (index, bytes) in bytes.chunks_mut(chunk).enumerate() {
            let file = &lease.file;
            threads.push(std::thread::Builder::new().spawn_scoped(scope, move || {
                let mut offset = (index * chunk) as u64;
                let mut remaining = bytes;
                while !remaining.is_empty() {
                    let read = read::at(file, remaining, offset)?;
                    if read == 0 {
                        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                    }
                    offset += read as u64;
                    remaining = &mut remaining[read..];
                }
                Ok(())
            })?);
        }
        for thread in threads {
            thread
                .join()
                .map_err(|_| io::Error::other("image read panicked"))??;
        }
        Ok::<_, io::Error>(())
    })?;
    drop(reading);
    let _hashing = kinakaze_v2_host_win::StartupSpan::begin("image-cache-hash");
    let bytes = unsafe { std::slice::from_raw_parts(view.0.Value.cast::<u8>(), length) };
    let hash = hash::hash(bytes);
    let mut previous = 0;
    if unsafe { VirtualProtect(view.0.Value, length, PAGE_READONLY, &mut previous) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((section, hash, view))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Seek, Write},
        os::windows::fs::OpenOptionsExt,
    };
    struct TestFile(std::path::PathBuf);
    impl TestFile {
        fn new() -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "kinakaze-image-cache-{}-{unique}",
                std::process::id()
            ));
            std::fs::write(&path, vec![7u8; MIN_SIZE]).unwrap();
            Self(path)
        }
        fn open(&self) -> File {
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(7)
                .custom_flags(FILE_FLAG_OVERLAPPED)
                .open(&self.0)
                .unwrap()
        }
    }
    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn birth() -> u64 {
        let mut t: [FILETIME; 4] = unsafe { std::mem::zeroed() };
        let p = t.as_mut_ptr();
        assert_ne!(
            unsafe { GetProcessTimes(GetCurrentProcess(), p, p.add(1), p.add(2), p.add(3)) },
            0
        );
        (u64::from(t[0].dwHighDateTime) << 32) | u64::from(t[0].dwLowDateTime)
    }
    fn get(cache: &mut Images, file: &File) -> Option<OwnedHandle> {
        cache
            .get(
                std::process::id(),
                birth(),
                file.as_raw_handle() as u64,
                MIN_SIZE as u64,
            )
            .map(|(h, hash)| {
                let owned = unsafe { owned(h as HANDLE).unwrap() };
                let view =
                    unsafe { MapViewOfFile(owned.as_raw_handle(), FILE_MAP_READ, 0, 0, MIN_SIZE) };
                assert!(!view.Value.is_null());
                let bytes =
                    unsafe { std::slice::from_raw_parts(view.Value.cast::<u8>(), MIN_SIZE) };
                assert_eq!(hash, *blake3::hash(bytes).as_bytes());
                unsafe {
                    UnmapViewOfFile(view);
                }
                owned
            })
    }
    fn check(handle: &OwnedHandle, first: u8) {
        let view = unsafe { MapViewOfFile(handle.as_raw_handle(), FILE_MAP_READ, 0, 0, MIN_SIZE) };
        assert!(!view.Value.is_null());
        let bytes = unsafe { std::slice::from_raw_parts(view.Value.cast::<u8>(), MIN_SIZE) };
        assert_eq!(bytes[0], first);
        assert!(bytes[1..].iter().all(|&v| v == 7));
        unsafe {
            UnmapViewOfFile(view);
        }
    }
    #[test]
    fn session_cache_survives_the_requesting_thread() {
        let path = TestFile::new();
        let file = path.open();
        let file_raw = file.as_raw_handle() as u64;
        let mut cache = Cache::default();
        let peer_birth = birth();
        let (mut cache, first) = std::thread::spawn(move || {
            let section = cache
                .get(std::process::id(), peer_birth, file_raw, MIN_SIZE as u64)
                .unwrap();
            (cache, unsafe { owned(section.0 as HANDLE).unwrap() })
        })
        .join()
        .unwrap();
        let second = unsafe {
            owned(
                cache
                    .get(std::process::id(), peer_birth, file_raw, MIN_SIZE as u64)
                    .unwrap()
                    .0 as HANDLE,
            )
            .unwrap()
        };
        assert_ne!(
            unsafe { CompareObjectHandles(first.as_raw_handle(), second.as_raw_handle()) },
            0
        );
        drop(cache);
        check(&first, 7);
        check(&second, 7);
    }
    #[test]
    fn read_lease_is_cancelled_when_its_issuing_thread_exits() {
        let path = TestFile::new();
        let file = path.open();
        let lease = std::thread::spawn(move || {
            let lease = Lease::new(file).unwrap();
            assert!(lease.valid());
            lease
        })
        .join()
        .unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(lease.event.as_raw_handle(), 1000) },
            WAIT_OBJECT_0
        );
        assert!(!lease.valid());
    }
    #[test]
    fn read_cache_reuses_only_unchanged_inode_and_preserves_prior_snapshots() {
        let path = TestFile::new();
        let file = path.open();
        let mut cache = Images::default();
        let first = get(&mut cache, &file).unwrap();
        check(&first, 7);
        let backing = cache.entries[0].section.as_raw_handle();
        let hit = get(&mut cache, &path.open()).unwrap();
        check(&hit, 7);
        assert_eq!(cache.entries[0].section.as_raw_handle(), backing);
        let stamp = file.metadata().unwrap().modified().unwrap();
        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .open(&path.0)
            .unwrap();
        writer.write_all(&[8]).unwrap();
        writer.flush().unwrap();
        writer
            .set_times(std::fs::FileTimes::new().set_modified(stamp))
            .unwrap();
        drop(writer);
        let changed = get(&mut cache, &path.open()).unwrap();
        check(&changed, 8);
        check(&first, 7);
        let renamed = path.0.with_extension("renamed");
        std::fs::rename(&path.0, &renamed).unwrap();
        std::fs::write(&path.0, vec![7u8; MIN_SIZE]).unwrap();
        check(&get(&mut cache, &file).unwrap(), 8);
        check(&get(&mut cache, &path.open()).unwrap(), 7);
        std::fs::remove_file(renamed).unwrap();
        check(&get(&mut cache, &file).unwrap(), 8);
    }
    #[test]
    fn transferred_section_allows_private_exec_but_no_shared_writes() {
        let path = TestFile::new();
        let mut cache = Images::default();
        let section = get(&mut cache, &path.open()).unwrap();
        let writable =
            unsafe { MapViewOfFile(section.as_raw_handle(), FILE_MAP_WRITE, 0, 0, MIN_SIZE) };
        assert!(writable.Value.is_null());
        let private = unsafe {
            MapViewOfFile(
                section.as_raw_handle(),
                FILE_MAP_COPY | FILE_MAP_EXECUTE,
                0,
                0,
                MIN_SIZE,
            )
        };
        assert!(!private.Value.is_null());
        unsafe {
            private.Value.cast::<u8>().write(9);
            UnmapViewOfFile(private);
        }
        check(&section, 7);
    }
    #[test]
    fn writable_handles_and_existing_writable_views_cannot_authorize_reuse() {
        let path = TestFile::new();
        let mut cache = Images::default();
        let mut writer = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path.0)
            .unwrap();
        assert!(get(&mut cache, &writer).is_none());
        let mapping = unsafe {
            owned(CreateFileMappingW(
                writer.as_raw_handle(),
                null(),
                PAGE_READWRITE,
                0,
                0,
                null(),
            ))
            .unwrap()
        };
        let view =
            unsafe { MapViewOfFile(mapping.as_raw_handle(), FILE_MAP_WRITE, 0, 0, MIN_SIZE) };
        assert!(!view.Value.is_null());
        assert!(get(&mut cache, &path.open()).is_none());
        unsafe {
            view.Value.cast::<u8>().write(8);
            UnmapViewOfFile(view);
        };
        drop(mapping);
        writer.seek(std::io::SeekFrom::Start(0)).unwrap();
        writer.write_all(&[9]).unwrap();
        drop(writer);
        check(&get(&mut cache, &path.open()).unwrap(), 9);
    }
}
