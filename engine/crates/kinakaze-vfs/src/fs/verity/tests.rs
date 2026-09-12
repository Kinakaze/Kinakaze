use super::*;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Storage::FileSystem::FILE_WRITE_ATTRIBUTES;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("kinakaze-verity-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn file(&self, name: &str, data: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, data).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fd(i32);
impl Fd {
    fn read(path: &Path) -> Self {
        Self(crate::fs::open(&crate::path::to_guest_path(path), crate::fs::O_RDONLY, 0).unwrap())
    }
    fn handle(&self) -> Object {
        Object::from_fd(self.0).unwrap()
    }
}
impl Drop for Fd {
    fn drop(&mut self) {
        crate::close(self.0).unwrap();
    }
}

#[test]
fn persisted_tree_masks_tail_and_survives_hardlink_unlink() {
    let f = Fixture::new();
    let data: Vec<u8> = (0..15_073).map(|i| (i * 71) as u8).collect();
    let path = f.file("content", &data);
    let alias = f.0.join("alias");
    std::fs::hard_link(&path, &alias).unwrap();
    let fd = Fd::read(&path);
    assert_eq!(measure(fd.0), Err(ENODATA));
    enable(fd.0, 1, 4096, b"inode test").unwrap();
    let original = fd.handle();
    let desc = descriptor(original.raw()).unwrap().unwrap();
    assert_eq!(desc.data_size(), data.len() as u64);
    assert_eq!(measure(fd.0).unwrap(), (1, desc.digest()));
    assert_eq!(enable(fd.0, 1, 4096, b""), Err(EEXIST));
    assert!(std::fs::metadata(&path).unwrap().len() > data.len() as u64);
    assert_eq!(
        crate::fs::stat(&crate::path::to_guest_path(&path))
            .unwrap()
            .st_size,
        data.len() as i64
    );
    assert_eq!(crate::fs::fstat(fd.0).unwrap().st_size, data.len() as i64);
    assert_eq!(
        authoritative_size(original.raw()).unwrap(),
        data.len() as u64
    );
    assert_eq!(ensure_writable(original.raw()), Err(EPERM));
    let linked = Fd::read(&alias);
    assert_eq!(measure(linked.0).unwrap(), measure(fd.0).unwrap());
    crate::fs::unlink(&crate::path::to_guest_path(&path)).unwrap();
    crate::fs::unlink(&crate::path::to_guest_path(&alias)).unwrap();
    std::fs::write(&path, b"unrelated replacement").unwrap();
    assert_eq!(crate::fs::fstat(fd.0).unwrap().st_size, data.len() as i64);
    assert_eq!(
        crate::fs::stat(&crate::path::to_guest_path(&path))
            .unwrap()
            .st_size,
        21
    );
    let mut bytes = vec![0xcc; data.len() + 8192];
    assert_eq!(
        verified_read(original.raw(), 0, &mut bytes).unwrap(),
        Some(data.len())
    );
    assert_eq!(&bytes[..data.len()], data);
    assert!(bytes[data.len()..].iter().all(|&v| v == 0xcc));
    assert_eq!(
        verified_read(original.raw(), data.len() as u64, &mut bytes).unwrap(),
        Some(0)
    );
    let mut raw_descriptor = [0u8; 300];
    assert_eq!(read_metadata(fd.0, 2, 0, &mut raw_descriptor).unwrap(), 256);
    assert_eq!(&raw_descriptor[..256], desc.bytes());
    assert_eq!(read_metadata(fd.0, 3, 0, &mut raw_descriptor), Err(ENODATA));
    assert_eq!(
        read_metadata(fd.0, 2, u64::MAX, &mut raw_descriptor),
        Err(EINVAL)
    );
}

#[test]
fn native_existing_writer_blocks_enable_without_changing_file() {
    let f = Fixture::new();
    let path = f.file("busy", b"original");
    let fd = Fd::read(&path);
    let writer = Object::open(&path, GENERIC_READ | GENERIC_WRITE).unwrap();
    assert_eq!(enable(fd.0, 1, 4096, b""), Err(ETXTBSY));
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert!(descriptor(writer.raw()).unwrap().is_none());
    drop(writer);
    enable(fd.0, 1, 4096, b"").unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 8);
}

#[test]
fn corruption_fails_before_unverified_bytes_are_returned() {
    for corrupt_tree in [false, true] {
        let f = Fixture::new();
        let data = vec![0x5a; 20_000];
        let path = f.file("corrupt", &data);
        let fd = Fd::read(&path);
        enable(fd.0, 2, 1024, b"salt").unwrap();
        let host_writer = Object::open(&path, GENERIC_READ | GENERIC_WRITE | QUERY_ACCESS).unwrap();
        let record = read_record(&host_writer).unwrap().unwrap();
        let offset = if corrupt_tree { record.tree_offset } else { 3 };
        exact_write(&host_writer, offset, &[0xff]).unwrap();
        host_writer.flush().unwrap();
        let mut bytes = [0xcc; 20];
        assert_eq!(verified_read(host_writer.raw(), 0, &mut bytes), Err(EIO));
        assert_eq!(bytes, [0xcc; 20]);
    }
}

#[test]
fn abandoned_preparing_tail_is_masked_and_recovered_before_write() {
    let f = Fixture::new();
    let path = f.file("abandoned", b"original payload");
    {
        let writer = Object::open(
            &path,
            GENERIC_READ | GENERIC_WRITE | QUERY_ACCESS | FILE_WRITE_EA,
        )
        .unwrap();
        let pending = Record::new(PREPARING, Descriptor::new(1, 4096, b"", 16).unwrap()).unwrap();
        ea::write(&writer, EA_NAME, &pending.encode()).unwrap();
        writer.flush().unwrap();
        exact_write(&writer, 4096, b"unpublished tree").unwrap();
        writer.flush().unwrap();
    }
    let fd = Fd::read(&path);
    let pin = fd.handle();
    assert_eq!(measure(fd.0), Err(ENODATA));
    assert_eq!(authoritative_size(pin.raw()).unwrap(), 16);
    assert_eq!(
        crate::fs::stat(&crate::path::to_guest_path(&path))
            .unwrap()
            .st_size,
        16
    );
    assert_eq!(crate::fs::fstat(fd.0).unwrap().st_size, 16);
    let mut bytes = [0xcc; 64];
    assert_eq!(verified_read(pin.raw(), 0, &mut bytes).unwrap(), Some(16));
    assert_eq!(&bytes[..16], b"original payload");
    ensure_writable(pin.raw()).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 16);
    assert_eq!(logical_size(pin.raw()).unwrap(), None);
    enable(fd.0, 1, 4096, b"").unwrap();
}

#[test]
fn malformed_persistent_state_fails_closed() {
    let f = Fixture::new();
    let path = f.file("bad-state", b"original");
    let object = Object::open(&path, GENERIC_READ | QUERY_ACCESS | FILE_WRITE_EA).unwrap();
    for bytes in [b"bad".as_slice(), &[0; 320]] {
        ea::write(&object, EA_NAME, bytes).unwrap();
        assert!(matches!(descriptor(object.raw()), Err(EIO)));
        assert_eq!(ensure_writable(object.raw()), Err(EIO));
        assert_eq!(verified_read(object.raw(), 0, &mut [0; 20]), Err(EIO));
        assert!(matches!(
            crate::fs::stat(&crate::path::to_guest_path(&path)),
            Err(EIO)
        ));
    }
}

#[test]
fn readonly_host_inode_is_not_temporarily_made_writable() {
    let f = Fixture::new();
    let path = f.file("readonly-marker", b"data");
    crate::fs::set_mode_host_path(&path, 0o444).unwrap();
    let query = Object::open(&path, QUERY_ACCESS | FILE_WRITE_EA | FILE_WRITE_ATTRIBUTES).unwrap();
    let fd = Fd::read(&path);
    // Native read-only projection is not bypassed by silently toggling host
    // attributes. A metadata-only handle cannot flush a durable transaction.
    let result = enable(fd.0, 1, 4096, b"");
    assert_eq!(query.flush(), Err(crate::EACCES));
    assert!(std::fs::metadata(&path).unwrap().permissions().readonly());
    assert!(logical_size(query.raw()).unwrap().is_none());
    crate::fs::set_mode_handle(query.raw(), 0o644).unwrap();
    assert_eq!(result, Err(crate::EACCES));
}

#[test]
fn granted_access_does_not_escalate_a_write_only_descriptor() {
    let f = Fixture::new();
    let path = f.file("write-only", b"secret");
    let writer = Object::open(&path, GENERIC_WRITE).unwrap();
    assert_eq!(verified_read(writer.raw(), 0, &mut [0; 20]), Err(EBADF));
    let fd =
        Fd(crate::fs::open(&crate::path::to_guest_path(&path), crate::fs::O_WRONLY, 0).unwrap());
    assert_eq!(enable(fd.0, 1, 4096, b""), Err(ETXTBSY));
}

#[test]
fn shared_writable_mapping_blocks_enable_after_fd_close() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
    };
    let f = Fixture::new();
    let path = f.file("mapped", &vec![0x42; 4096]);
    let writer = Object::open(&path, GENERIC_READ | GENERIC_WRITE).unwrap();
    let section = unsafe {
        CreateFileMappingW(
            writer.raw(),
            std::ptr::null(),
            PAGE_READWRITE,
            0,
            0,
            std::ptr::null(),
        )
    };
    assert!(!section.is_null());
    let view = unsafe { MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, 4096) };
    assert!(!view.Value.is_null());
    drop(writer);
    let fd = Fd::read(&path);
    let result = enable(fd.0, 1, 4096, b"");
    unsafe {
        UnmapViewOfFile(view);
        CloseHandle(section);
    }
    assert_eq!(result, Err(ETXTBSY));
    enable(fd.0, 1, 4096, b"").unwrap();
}

// This native child exists only for the test and exits without Rust destructors;
// it is not a filesystem service or part of the production ownership scheme.
#[test]
fn abandoned_transaction_process_helper() {
    let Some(path) = std::env::var_os("KINAKAZE_VERITY_CRASH_FIXTURE") else {
        return;
    };
    let writer = Object::open(
        Path::new(&path),
        GENERIC_READ | GENERIC_WRITE | QUERY_ACCESS | FILE_WRITE_EA,
    )
    .unwrap();
    let _lock = lock(writer.raw()).unwrap();
    let descriptor = Descriptor::new(1, 4096, b"crash", native_size(&writer).unwrap()).unwrap();
    let pending = Record::new(PREPARING, descriptor).unwrap();
    ea::write(&writer, EA_NAME, &pending.encode()).unwrap();
    writer.flush().unwrap();
    exact_write(&writer, pending.tree_offset, &vec![0x93; 8192]).unwrap();
    writer.flush().unwrap();
    std::process::exit(37);
}

#[test]
fn process_death_releases_inode_guard_and_next_process_recovers_tail() {
    use std::os::windows::process::CommandExt;
    let f = Fixture::new();
    let data = b"durable original file";
    let path = f.file("crash", data);
    let test = format!(
        "{}::abandoned_transaction_process_helper",
        module_path!().split_once("::").unwrap().1
    );
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test, "--nocapture"])
        .env("KINAKAZE_VERITY_CRASH_FIXTURE", &path)
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(37),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(std::fs::metadata(&path).unwrap().len() > data.len() as u64);
    let fd = Fd::read(&path);
    let object = fd.handle();
    assert_eq!(authoritative_size(object.raw()).unwrap(), data.len() as u64);
    let mut buffer = [0xcc; 1024];
    assert_eq!(
        verified_read(object.raw(), 0, &mut buffer).unwrap(),
        Some(data.len())
    );
    assert_eq!(&buffer[..data.len()], data);
    ensure_writable(object.raw()).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), data);
    assert!(logical_size(object.raw()).unwrap().is_none());
    enable(fd.0, 2, 2048, b"recovered").unwrap();
    assert_eq!(measure(fd.0).unwrap().0, 2);
}

#[test]
fn hidden_merkle_writes_do_not_change_guest_mtime() {
    let f = Fixture::new();
    let path = f.file("timestamps", &vec![0x46; 20_000]);
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    let fd = Fd::read(&path);
    enable(fd.0, 1, 4096, b"").unwrap();
    drop(fd);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before
    );
}

#[test]
fn directory_ioctl_errors_distinguish_enable_from_measure() {
    let f = Fixture::new();
    let fd = Fd::read(&f.0);
    assert_eq!(enable(fd.0, 1, 4096, b""), Err(EISDIR));
    assert_eq!(measure(fd.0), Err(ENODATA));
    assert_eq!(read_metadata(fd.0, 1, 0, &mut [0; 20]), Err(ENODATA));
}

#[test]
fn opened_metadata_stays_on_one_inode_across_close_and_slot_reuse() {
    use std::os::windows::io::IntoRawHandle;
    let fixture = Fixture::new();
    let first = fixture.file("first", &vec![0x35; 1024 * 1024]);
    let second = fixture.file("second", &vec![0x96; 1024 * 1024]);
    let fd = Fd::read(&first);
    let replacement = Fd::read(&second);
    enable(fd.0, 1, 1024, b"first").unwrap();
    enable(replacement.0, 1, 1024, b"second").unwrap();
    let opened = Opened::from_fd(fd.0).unwrap();
    let measured = opened.measure().unwrap();
    let descriptor = opened.descriptor().unwrap().unwrap();
    assert_ne!(measured, measure(replacement.0).unwrap());
    let mut expected = vec![0; descriptor.tree_size().unwrap() as usize];
    assert!(expected.len() > 16 * 1024);
    assert_eq!(
        opened.read_metadata(1, 0, &mut expected),
        Ok(expected.len())
    );

    let mut actual = vec![0; expected.len()];
    let mut done = opened
        .read_metadata(1, 0, &mut actual[..16 * 1024])
        .unwrap();
    crate::close(fd.0).unwrap();
    // Even with no descriptor referring to it, the operation owns its inode.
    assert_eq!(opened.measure().unwrap(), measured);
    let raw = std::fs::File::open(&second).unwrap().into_raw_handle();
    crate::install_exact(
        raw as usize,
        FdKind::File,
        FdFlags::READ_ACCESS.union(FdFlags::SEEKABLE),
        fd.0,
    )
    .unwrap();
    assert_ne!(crate::get(fd.0).unwrap().generation, opened.generation());
    assert_ne!(measure(fd.0).unwrap(), measured);
    while done < actual.len() {
        let end = actual.len().min(done + 16 * 1024);
        let count = opened
            .read_metadata(1, done as u64, &mut actual[done..end])
            .unwrap();
        assert_ne!(count, 0);
        done += count;
    }
    assert_eq!(
        actual, expected,
        "metadata chunks cannot switch to the recycled fd"
    );
    assert_eq!(
        opened.descriptor().unwrap().unwrap().bytes(),
        descriptor.bytes()
    );
    assert_eq!(opened.measure().unwrap(), measured);
}

#[test]
fn opened_validates_path_only_before_descriptor_kind() {
    let fixture = Fixture::new();
    let path = fixture.file("path-only", b"metadata");
    let fd = Fd(crate::fs::open(&crate::path::to_guest_path(&path), crate::fs::O_PATH, 0).unwrap());
    assert_eq!(Opened::from_fd(fd.0).unwrap_err(), EBADF);
    let special = Fd(crate::install_dev_special(FdKind::Null, FdFlags::PATH_ONLY).unwrap());
    assert_eq!(Opened::from_fd(special.0).unwrap_err(), EBADF);
    let ordinary = Fd(crate::install_dev_special(FdKind::Null, FdFlags::READ_ACCESS).unwrap());
    assert_eq!(Opened::from_fd(ordinary.0).unwrap_err(), ENOTTY);
}

#[test]
#[ignore = "native capability diagnostic; does not define guest fs-verity behavior"]
fn native_readonly_mapping_same_eof_probe() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_READ, MapViewOfFile, PAGE_READONLY, UnmapViewOfFile,
    };
    let f = Fixture::new();
    let path = f.file("read-mapped", &vec![0x42; 4096]);
    let reader = Object::open(&path, GENERIC_READ).unwrap();
    let section = unsafe {
        CreateFileMappingW(
            reader.raw(),
            std::ptr::null(),
            PAGE_READONLY,
            0,
            0,
            std::ptr::null(),
        )
    };
    assert!(!section.is_null());
    let view = unsafe { MapViewOfFile(section, FILE_MAP_READ, 0, 0, 4096) };
    assert!(!view.Value.is_null());
    drop(reader);
    let writer = Object::open(&path, GENERIC_READ | GENERIC_WRITE).unwrap();
    let result = writer.set_length(4096);
    eprintln!("native same-EOF with readonly mapped view: {result:?}");
    unsafe {
        UnmapViewOfFile(view);
        CloseHandle(section);
    }
    assert_eq!(native_size(&writer).unwrap(), 4096);
}
