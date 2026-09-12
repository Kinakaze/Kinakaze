use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kinakaze-proc-fd-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        crate::to_guest_path(&path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Descriptor(i32);
impl Drop for Descriptor {
    fn drop(&mut self) {
        crate::close(self.0).unwrap();
    }
}
fn magic(fd: i32) -> String {
    format!("/proc/self/fd/{fd}")
}

#[test]
fn proc_stat_opath_reopens_with_stable_process_identity_and_new_offset() {
    let source = Descriptor(open("/proc/self/stat", O_PATH, 0).unwrap());
    let expected = crate::procfs::read_file("/proc/self/stat").unwrap();
    let fresh = Descriptor(open(&magic(source.0), O_RDONLY | O_CLOEXEC, 0).unwrap());
    let entry = crate::get(fresh.0).unwrap();
    assert!(!entry.flags.contains(FdFlags::PATH_ONLY));
    assert!(entry.flags.contains(FdFlags::CLOSE_ON_EXEC));
    assert_eq!(crate::read(source.0, &mut [0; 8]), Err(EBADF));
    let mut bytes = [0; 4096];
    assert_eq!(crate::read(fresh.0, &mut bytes[..7]), Ok(7));
    let independent = Descriptor(open(&magic(fresh.0), O_RDONLY, 0).unwrap());
    drop(source);
    let count = crate::read(independent.0, &mut bytes).unwrap();
    assert_eq!(&bytes[..7], &expected[..7]);
    assert_eq!(lseek(fresh.0, 0, SEEK_CUR), Ok(7));
    let stat = String::from_utf8(bytes[..count].to_vec()).unwrap();
    let fields: Vec<_> = stat[stat.rfind(')').unwrap() + 2..]
        .split_whitespace()
        .collect();
    assert!(fields[19].parse::<u64>().unwrap() > 0);
    let original = String::from_utf8(expected).unwrap();
    let original_fields: Vec<_> = original[original.rfind(')').unwrap() + 2..]
        .split_whitespace()
        .collect();
    assert_eq!(fields[19], original_fields[19]);
}

#[test]
fn proc_setgroups_opath_reopens_for_data_with_independent_flags_and_offset() {
    let source = Descriptor(open("/proc/self/setgroups", O_PATH | O_RDWR | O_TRUNC, 0).unwrap());
    assert!(
        crate::get(source.0)
            .unwrap()
            .flags
            .contains(FdFlags::PATH_ONLY)
    );
    assert_eq!(crate::read(source.0, &mut [0; 8]), Err(EBADF));
    assert_eq!(crate::write(source.0, b"deny"), Err(EBADF));
    let fresh = Descriptor(open(&magic(source.0), O_RDONLY | O_CLOEXEC, 0).unwrap());
    let entry = crate::get(fresh.0).unwrap();
    assert_ne!(
        entry.description_id,
        crate::get(source.0).unwrap().description_id
    );
    assert!(entry.flags.contains(FdFlags::READ_ACCESS));
    assert!(entry.flags.contains(FdFlags::CLOSE_ON_EXEC));
    assert!(!entry.flags.contains(FdFlags::PATH_ONLY));
    let mut bytes = [0; 8];
    assert_eq!(crate::read(fresh.0, &mut bytes[..2]), Ok(2));
    assert_eq!(&bytes[..2], b"al");
    let independent = Descriptor(open(&magic(fresh.0), O_RDONLY, 0).unwrap());
    assert_eq!(crate::read(independent.0, &mut bytes), Ok(6));
    assert_eq!(&bytes[..6], b"allow\n");
    assert_eq!(lseek(fresh.0, 0, SEEK_CUR), Ok(2));
    assert_eq!(crate::write(fresh.0, b"deny"), Err(EBADF));
    drop(source);
    assert_eq!(crate::read(fresh.0, &mut bytes), Ok(4));
    assert_eq!(&bytes[..4], b"low\n");
}

#[test]
fn proc_control_reopen_validates_access_and_uses_existing_write_handler() {
    let source = Descriptor(open("/proc/self/uid_map", O_PATH, 0).unwrap());
    assert_eq!(open(&magic(source.0), O_DIRECTORY, 0), Err(ENOTDIR));
    assert_eq!(open(&magic(source.0), O_NOFOLLOW, 0), Err(ELOOP));
    assert_eq!(open(&magic(source.0), O_ACCMODE, 0), Err(EINVAL));
    assert_eq!(
        open(&magic(source.0), O_CREAT | O_EXCL, 0),
        Err(crate::EEXIST)
    );
    let writer = Descriptor(open(&magic(source.0), O_WRONLY, 0).unwrap());
    assert_eq!(crate::read(writer.0, &mut [0; 8]), Err(EBADF));
    // The initial namespace map cannot be replaced, even through a procfd alias.
    assert_eq!(crate::write(writer.0, b"0 0 1\n"), Err(crate::EPERM));
    let path = Descriptor(open(&magic(writer.0), O_PATH | O_WRONLY | O_NONBLOCK, 0).unwrap());
    let flags = crate::get(path.0).unwrap().flags;
    assert!(flags.contains(FdFlags::PATH_ONLY));
    assert!(!flags.contains(FdFlags::WRITE_ACCESS));
    assert!(!flags.contains(FdFlags::NONBLOCK));
}

#[test]
fn regular_reopen_has_new_access_flags_and_independent_position() {
    let fixture = Fixture::new();
    let path = fixture.file("regular", b"abcdef");
    let original = Descriptor(open(&path, O_RDONLY | O_NONBLOCK, 0).unwrap());
    assert_eq!(lseek(original.0, 3, SEEK_SET), Ok(3));
    let fresh = Descriptor(open(&magic(original.0), O_RDWR | O_CLOEXEC, 0).unwrap());
    let source = crate::get(original.0).unwrap();
    let target = crate::get(fresh.0).unwrap();
    assert_ne!(source.description_id, target.description_id);
    assert!(target.flags.contains(FdFlags::READ_ACCESS));
    assert!(target.flags.contains(FdFlags::WRITE_ACCESS));
    assert!(target.flags.contains(FdFlags::CLOSE_ON_EXEC));
    assert!(!target.flags.contains(FdFlags::NONBLOCK));
    let mut byte = [0u8; 1];
    assert_eq!(crate::read(fresh.0, &mut byte), Ok(1));
    assert_eq!(byte, [b'a']);
    assert_eq!(lseek(original.0, 0, SEEK_CUR), Ok(3));
    assert_eq!(crate::write(fresh.0, b"!"), Ok(1));
    assert_eq!(std::fs::read(fixture.0.join("regular")).unwrap(), b"a!cdef");
    // Numeric and self aliases are the same procfs namespace lookup.
    let numeric = format!("/proc/{}/fd/{}", crate::job::process_id(), original.0);
    let numeric = Descriptor(open(&numeric, O_RDONLY, 0).unwrap());
    assert_eq!(lseek(numeric.0, 0, SEEK_CUR), Ok(0));
}

#[test]
fn reopen_retains_unlinked_inode_and_opath_directory() {
    let fixture = Fixture::new();
    let path = fixture.file("original", b"retained");
    let original = Descriptor(open(&path, O_PATH, 0).unwrap());
    unlink(&path).unwrap();
    fixture.file("original", b"replacement");
    let fresh = Descriptor(open(&magic(original.0), O_RDONLY, 0).unwrap());
    let mut bytes = [0u8; 16];
    let count = crate::read(fresh.0, &mut bytes).unwrap();
    assert_eq!(&bytes[..count], b"retained");
    let directory =
        Descriptor(open(&crate::to_guest_path(&fixture.0), O_PATH | O_DIRECTORY, 0).unwrap());
    let reopened = Descriptor(open(&magic(directory.0), O_DIRECTORY, 0).unwrap());
    assert_eq!(crate::get(reopened.0).unwrap().kind, FdKind::Directory);
    assert!(
        !crate::get(reopened.0)
            .unwrap()
            .flags
            .contains(FdFlags::PATH_ONLY)
    );
}

#[test]
fn failed_native_write_reopen_does_not_duplicate_read_handle() {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::IntoRawHandle;
    let fixture = Fixture::new();
    fixture.file("restricted", b"unchanged");
    let host = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(fixture.0.join("restricted"))
        .unwrap();
    let source = Descriptor(
        install(
            host.into_raw_handle() as usize,
            FdKind::File,
            FdFlags::READ_ACCESS.union(FdFlags::SEEKABLE),
        )
        .unwrap(),
    );
    assert!(open(&magic(source.0), O_WRONLY, 0).is_err());
    assert!(open(&magic(source.0), O_WRONLY | O_TRUNC, 0).is_err());
    assert_eq!(
        std::fs::read(fixture.0.join("restricted")).unwrap(),
        b"unchanged"
    );
}

#[test]
fn pipe_reopen_creates_independent_description_on_same_endpoint() {
    let (reader, writer) = crate::create_pipe(FdFlags::NONBLOCK, 4096).unwrap();
    let reader = Descriptor(reader);
    let writer = Descriptor(writer);
    let reopened_reader = Descriptor(open(&magic(reader.0), O_RDONLY | O_CLOEXEC, 0).unwrap());
    let reopened_writer = Descriptor(open(&magic(writer.0), O_WRONLY, 0).unwrap());
    let source = crate::get(reader.0).unwrap();
    let fresh = crate::get(reopened_reader.0).unwrap();
    assert_ne!(source.description_id, fresh.description_id);
    assert!(!fresh.flags.contains(FdFlags::NONBLOCK));
    assert!(source.flags.contains(FdFlags::NONBLOCK));
    assert!(fresh.flags.contains(FdFlags::CLOSE_ON_EXEC));
    assert_eq!(crate::write(reopened_reader.0, b"wrong-end"), Err(EBADF));
    assert_eq!(crate::read(reopened_writer.0, &mut [0u8]), Err(EBADF));
    drop(reader);
    drop(writer);
    assert_eq!(crate::write(reopened_writer.0, b"same queue"), Ok(10));
    let mut bytes = [0u8; 10];
    assert_eq!(crate::read(reopened_reader.0, &mut bytes), Ok(10));
    assert_eq!(&bytes, b"same queue");
    assert_eq!(lseek(reopened_reader.0, 0, SEEK_CUR), Err(ESPIPE));
}

#[test]
fn proc_magic_link_rejects_missing_fd_and_wrong_directory_flags() {
    let fixture = Fixture::new();
    let path = fixture.file("regular", b"data");
    let source = Descriptor(open(&path, O_RDONLY, 0).unwrap());
    assert_eq!(
        open(&magic(source.0), O_RDONLY | O_DIRECTORY, 0),
        Err(ENOTDIR)
    );
    assert_eq!(open(&magic(source.0), O_RDONLY | O_NOFOLLOW, 0), Err(ELOOP));
    assert_eq!(
        open(&magic(source.0), O_CREAT | O_EXCL | O_RDWR, 0),
        Err(crate::EEXIST)
    );
    let allowed = Descriptor(open(&magic(source.0), O_CREAT | O_RDONLY, 0).unwrap());
    assert_eq!(crate::get(allowed.0).unwrap().kind, FdKind::File);
    assert_eq!(open("/proc/self/fd/2147483647", O_RDONLY, 0), Err(ENOENT));
    assert_eq!(open("/proc/self/fd/-1", O_RDONLY, 0), Err(ENOENT));
}

#[test]
fn anon_inode_open_is_not_a_handle_duplicate() {
    let counter = Descriptor(crate::eventfd::create_eventfd(7, 0).unwrap());
    assert_eq!(open(&magic(counter.0), O_RDONLY, 0), Err(crate::ENXIO));
    let mut value = [0u8; 8];
    assert_eq!(crate::read(counter.0, &mut value), Ok(8));
    assert_eq!(u64::from_ne_bytes(value), 7);
}

#[test]
fn proc_reopen_does_not_bypass_verity_write_protection() {
    let fixture = Fixture::new();
    let path = fixture.file("verified", b"authenticated");
    let source = Descriptor(open(&path, O_RDONLY, 0).unwrap());
    verity::enable(source.0, 1, 4096, &[]).unwrap();
    assert_eq!(open(&magic(source.0), O_RDWR, 0), Err(crate::EPERM));
    assert_eq!(
        open(&magic(source.0), O_WRONLY | O_TRUNC, 0),
        Err(crate::EPERM)
    );
    let readable = Descriptor(open(&magic(source.0), O_RDONLY, 0).unwrap());
    let mut bytes = [0u8; 64];
    let count = crate::read(readable.0, &mut bytes).unwrap();
    assert_eq!(&bytes[..count], b"authenticated");
}

#[test]
fn pinned_fifo_rdwr_must_share_the_named_fifo_queue() {
    let fixture = Fixture::new();
    let path = fixture.file("fifo", b"");
    set_mode_host_path(&fixture.0.join("fifo"), S_IFIFO | 0o600).unwrap();
    let marker = Descriptor(open(&path, O_PATH, 0).unwrap());
    let loopback = Descriptor(open(&magic(marker.0), O_RDWR | O_NONBLOCK, 0).unwrap());
    assert_eq!(crate::write(loopback.0, b"self"), Ok(4));
    let mut bytes = [0u8; 4];
    assert_eq!(crate::read(loopback.0, &mut bytes), Ok(4));
    assert_eq!(&bytes, b"self");
    let external_writer = Descriptor(open(&path, O_WRONLY | O_NONBLOCK, 0).unwrap());
    assert_eq!(crate::write(external_writer.0, b"peer"), Ok(4));
    assert_eq!(crate::read(loopback.0, &mut bytes), Ok(4));
    assert_eq!(&bytes, b"peer");
}

#[test]
fn fifo_marker_reopen_preserves_inode_across_rename_unlink_and_path_replacement() {
    let fixture = Fixture::new();
    let path = fixture.file("fifo", b"");
    set_mode_host_path(&fixture.0.join("fifo"), S_IFIFO | 0o600).unwrap();
    let marker = Descriptor(open(&path, O_PATH, 0).unwrap());
    let endpoint = Descriptor(open(&magic(marker.0), O_RDWR | O_NONBLOCK, 0).unwrap());
    let moved = crate::to_guest_path(&fixture.0.join("renamed"));
    rename(&path, &moved).unwrap();
    let reader = Descriptor(open(&moved, O_RDONLY | O_NONBLOCK, 0).unwrap());
    assert_eq!(crate::write(endpoint.0, b"before"), Ok(6));
    assert_eq!(crate::read(reader.0, &mut [0; 6]), Ok(6));
    unlink(&moved).unwrap();
    let replacement = fixture.file("renamed", b"");
    set_mode_host_path(&fixture.0.join("renamed"), S_IFIFO | 0o600).unwrap();
    let distinct = Descriptor(open(&replacement, O_RDWR | O_NONBLOCK, 0).unwrap());
    let retained = Descriptor(open(&magic(marker.0), O_WRONLY | O_NONBLOCK, 0).unwrap());
    assert_eq!(crate::write(retained.0, b"retained"), Ok(8));
    let mut bytes = [0; 8];
    assert_eq!(crate::read(reader.0, &mut bytes), Ok(8));
    assert_eq!(&bytes, b"retained");
    assert_eq!(crate::read(distinct.0, &mut bytes), Err(crate::EAGAIN));
    assert_ne!(
        fstat(distinct.0).unwrap().st_ino,
        fstat(endpoint.0).unwrap().st_ino
    );
    assert_eq!(fstat(endpoint.0).unwrap().st_nlink, 0);
}

#[test]
fn procfs_root_and_process_identity_agree_for_paths_and_duplicate_descriptors() {
    let root = Descriptor(open("/proc", O_PATH | O_DIRECTORY | O_NOFOLLOW, 0).unwrap());
    let entry = crate::get(root.0).unwrap();
    let alias = Descriptor(crate::install(0, entry.kind, entry.flags).unwrap());
    crate::duplicate_synthetic_description(root.0, alias.0).unwrap();
    let reopened = Descriptor(open(&magic(root.0), O_DIRECTORY | O_CLOEXEC, 0).unwrap());
    assert_ne!(
        crate::get(root.0).unwrap().description_id,
        crate::get(reopened.0).unwrap().description_id
    );
    assert!(
        crate::get(reopened.0)
            .unwrap()
            .flags
            .contains(FdFlags::CLOSE_ON_EXEC)
    );
    let path = stat("/proc").unwrap();
    assert_eq!(path.st_ino, 1);
    for fd in [root.0, alias.0, reopened.0] {
        let st = fstat(fd).unwrap();
        assert_eq!(
            (st.st_dev, st.st_ino, st.st_mode),
            (path.st_dev, 1, path.st_mode)
        );
    }
    let own = Descriptor(open("/proc/self", O_DIRECTORY, 0).unwrap());
    let numeric = format!("/proc/{}", crate::job::process_id());
    assert_eq!(crate::synthetic_directory_path(own.0).unwrap(), numeric);
    assert_eq!(fstat(own.0).unwrap().st_ino, stat(&numeric).unwrap().st_ino);
    assert_ne!(fstat(own.0).unwrap().st_ino, 1);
}
