use super::*;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtReadFile(
        file: HANDLE,
        event: HANDLE,
        apc: *const u8,
        context: *const u8,
        io: *mut NativeIoStatus,
        buffer: *mut u8,
        length: u32,
        offset: *const i64,
        key: *const u32,
    ) -> i32;
}

struct Pipe(usize, usize);
impl Pipe {
    fn new() -> Self {
        let (read, write) = crate::platform::create_overlapped_pipe_pair(4096).unwrap();
        Self(read, write)
    }
    fn send_later(&self) -> std::thread::JoinHandle<()> {
        let raw = self.1;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let entry = crate::FdEntry {
                raw,
                kind: crate::FdKind::Pipe,
                flags: crate::FdFlags::OVERLAPPED,
                generation: 0,
                description_id: 0,
                offset: 0,
            };
            let mut byte = b'q';
            assert_eq!(
                unsafe { crate::platform::transfer_once(&entry, &mut byte, 1, false) },
                Ok(1)
            );
        })
    }
    fn submit(&self, io: &mut NativeIoStatus, byte: &mut u8) -> i32 {
        io.status = 0x103;
        unsafe {
            NtReadFile(
                self.0 as _,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                io,
                byte,
                1,
                ptr::null(),
                ptr::null(),
            )
        }
    }
}
impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0 as _);
            CloseHandle(self.1 as _);
        }
    }
}

static SIGNALS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "sysv64" fn handler(_: i32) {
    SIGNALS.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn an_earlier_completion_event_does_not_complete_the_pending_request() {
    let _serial = crate::signal::test_lock();
    assert!(!crate::interrupt::current().is_null());
    let pipe = Pipe::new();
    let (mut first, mut second) = (NativeIoStatus::default(), NativeIoStatus::default());
    let (mut first_byte, mut second_byte) = (0u8, 0u8);
    assert_eq!(pipe.submit(&mut first, &mut first_byte), 0x103);
    assert_eq!(pipe.submit(&mut second, &mut second_byte), 0x103);
    let entry = crate::FdEntry {
        raw: pipe.1,
        kind: crate::FdKind::Pipe,
        flags: crate::FdFlags::OVERLAPPED,
        generation: 0,
        description_id: 0,
        offset: 0,
    };
    let mut byte = b'a';
    assert_eq!(
        unsafe { crate::platform::transfer_once(&entry, &mut byte, 1, false) },
        Ok(1)
    );
    assert_eq!(unsafe { complete(pipe.0 as _, &mut first, 0x103) }, Ok(0));
    assert_eq!(first_byte, b'a');
    assert_eq!(unsafe { ptr::read_volatile(&second.status) }, 0x103);
    assert_eq!(
        unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(pipe.0 as _, 0) },
        windows_sys::Win32::Foundation::WAIT_OBJECT_0
    );
    let writer = pipe.send_later();
    assert_eq!(unsafe { complete(pipe.0 as _, &mut second, 0x103) }, Ok(0));
    writer.join().unwrap();
    assert_eq!(second.information, 1);
    assert_eq!(second_byte, b'q');
}

#[test]
fn cancellation_preserves_another_pending_request_on_the_same_open() {
    let _serial = crate::signal::test_lock();
    SIGNALS.store(0, Ordering::SeqCst);
    assert!(!crate::interrupt::current().is_null());
    let previous = crate::signal::sigaction(
        12,
        Some(crate::signal::Action {
            disposition: crate::signal::Disposition::Handle(handler, 0),
            ..crate::signal::Action::default()
        }),
    )
    .unwrap();
    let mask = crate::signal::swap_blocked_mask(0);
    let pipe = Pipe::new();
    let (mut first, mut second) = (NativeIoStatus::default(), NativeIoStatus::default());
    let (mut first_byte, mut second_byte) = (0u8, 0u8);
    assert_eq!(pipe.submit(&mut first, &mut first_byte), 0x103);
    assert_eq!(pipe.submit(&mut second, &mut second_byte), 0x103);
    crate::signal::raise_thread_signal(crate::interrupt::current_thread_id(), 12).unwrap();
    assert_eq!(
        unsafe { complete(pipe.0 as _, &mut first, 0x103) }.unwrap() as u32,
        0xc000_0120
    );
    assert_eq!(SIGNALS.load(Ordering::SeqCst), 0);
    assert_eq!(unsafe { ptr::read_volatile(&second.status) }, 0x103);
    crate::signal::deliver_pending();
    assert_eq!(SIGNALS.load(Ordering::SeqCst), 1);
    let writer = pipe.send_later();
    assert_eq!(unsafe { complete(pipe.0 as _, &mut second, 0x103) }, Ok(0));
    writer.join().unwrap();
    assert_eq!(second.information, 1);
    assert_eq!(second_byte, b'q');
    crate::signal::swap_blocked_mask(mask);
    crate::signal::sigaction(12, Some(previous)).unwrap();
}

#[test]
fn named_metadata_queries_preserve_offset_and_work_with_limited_rights() {
    use super::super::{ea, inode, object::Object, verity};
    use std::io::{Seek, SeekFrom};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_WRITE_EA,
    };
    let path = std::env::temp_dir().join(format!(
        "kinakaze-shared-query-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, b"retained inode contents").unwrap();
    let metadata =
        Object::open(&path, FILE_READ_ATTRIBUTES | FILE_READ_EA | FILE_WRITE_EA).unwrap();
    let mut record = inode::Record::default();
    record.mode = Some(0o100640);
    record.uid = Some(1234);
    record.gid = Some(5678);
    ea::write(&metadata, inode::EA_NAME, &record.encode().unwrap()).unwrap();
    for rights in [GENERIC_READ, FILE_READ_ATTRIBUTES, GENERIC_WRITE] {
        let handle = Object::open(&path, rights).unwrap();
        for _ in 0..32 {
            let found = inode::read(handle.raw()).unwrap();
            assert_eq!(found.mode, record.mode);
            assert_eq!(found.uid, record.uid);
            assert_eq!(found.gid, record.gid);
            assert!(verity::descriptor(handle.raw()).unwrap().is_none());
            assert_eq!(verity::logical_size(handle.raw()).unwrap(), None);
        }
    }
    let reader = Object::open(&path, GENERIC_READ).unwrap();
    let mut positioned = std::fs::File::open(&path).unwrap();
    positioned.seek(SeekFrom::Start(9)).unwrap();
    assert_eq!(
        inode::read(positioned.as_raw_handle()).unwrap().uid,
        Some(1234)
    );
    assert_eq!(positioned.stream_position().unwrap(), 9);
    drop(positioned);
    let moved = path.with_extension("moved");
    std::fs::rename(&path, &moved).unwrap();
    std::fs::write(&path, b"replacement").unwrap();
    std::fs::remove_file(&moved).unwrap();
    assert_eq!(inode::read(reader.raw()).unwrap().uid, Some(1234));
    let mut bytes = [0u8; 4];
    assert_eq!(reader.read_at(9, &mut bytes), Ok(4));
    assert_eq!(&bytes, b"inod");
    for flags in [
        crate::FdFlags::OVERLAPPED,
        crate::FdFlags::OVERLAPPED.union(crate::FdFlags::SEEKABLE),
    ] {
        let entry = crate::FdEntry {
            raw: reader.raw() as usize,
            kind: crate::FdKind::File,
            flags,
            generation: 0,
            description_id: 0,
            offset: 9,
        };
        bytes.fill(0);
        assert_eq!(verity::verified_read_entry(&entry, &mut bytes), Ok(Some(4)));
        assert_eq!(&bytes, b"inod");
    }
    assert_eq!(inode::read(reader.raw()).unwrap().gid, Some(5678));
    drop(reader);
    drop(metadata);
    std::fs::remove_file(path).unwrap();
}
