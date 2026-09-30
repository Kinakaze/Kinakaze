use super::*;
use crate::{FdKind, fs};
use std::os::windows::{fs::OpenOptionsExt, io::IntoRawHandle};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE,
};

#[test]
fn native_data_hole_seek_preserves_offset_on_failure() {
    let path = std::env::temp_dir().join(format!("kinakaze-hole-seek-{}.tmp", std::process::id()));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(&path)
        .unwrap();
    file.set_len(1152).unwrap();
    let fd = crate::install(
        file.into_raw_handle() as usize,
        FdKind::File,
        FdFlags::SEEKABLE
            .union(FdFlags::READ_ACCESS)
            .union(FdFlags::WRITE_ACCESS),
    )
    .unwrap();
    assert_eq!(fs::lseek(fd, 384, 3), Ok(384));
    assert_eq!(fs::lseek(fd, 384, 4), Ok(1152));
    for (offset, error) in [
        (-1, crate::EINVAL),
        (1152, crate::ENXIO),
        (2000, crate::ENXIO),
    ] {
        for whence in [3, 4] {
            assert_eq!(fs::lseek(fd, offset, whence), Err(error));
            assert_eq!(fs::lseek(fd, 0, fs::SEEK_CUR), Ok(1152));
        }
    }
    crate::close(fd).unwrap();
}

#[test]
fn promoted_alias_publishes_its_completed_write_read_and_seek_position() {
    let path = std::env::temp_dir().join(format!(
        "kinakaze-ofd-alias-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OVERLAPPED | FILE_FLAG_DELETE_ON_CLOSE)
        .open(&path)
        .unwrap();
    let flags = FdFlags::OVERLAPPED
        .union(FdFlags::SEEKABLE)
        .union(FdFlags::READ_ACCESS)
        .union(FdFlags::WRITE_ACCESS);
    let fd = crate::install(
        file.try_clone().unwrap().into_raw_handle() as usize,
        FdKind::File,
        flags,
    )
    .unwrap();
    let alias = crate::install_duplicate(
        file.into_raw_handle() as usize,
        FdKind::File,
        flags,
        crate::get(fd).unwrap(),
    )
    .unwrap();
    assert!(alias > fd);
    assert_eq!(crate::write(fd, b"A"), Ok(1));
    let shared = promote(fd).unwrap();
    for expected in 2..=3 {
        assert_eq!(crate::write(alias, b"B"), Ok(1));
        assert_eq!(crate::get(fd).unwrap().offset, expected);
        assert_eq!(crate::get(alias).unwrap().offset, expected);
        assert_eq!(
            decode(&shared.read().unwrap().1, crate::get(fd).unwrap())
                .unwrap()
                .offset,
            expected
        );
    }
    assert_eq!(fs::lseek(alias, 0, fs::SEEK_SET), Ok(0));
    let mut bytes = [0; 3];
    assert_eq!(crate::read(alias, &mut bytes[..1]), Ok(1));
    assert_eq!(crate::get(fd).unwrap().offset, 1);
    assert_eq!(crate::read(fd, &mut bytes[1..]), Ok(2));
    assert_eq!(&bytes, b"ABB");
    assert_eq!(fs::lseek(alias, 0, fs::SEEK_CUR), Ok(3));
    crate::close(fd).unwrap();
    assert_eq!(crate::write(alias, b"C"), Ok(1));
    assert_eq!(fs::lseek(alias, 0, fs::SEEK_SET), Ok(0));
    let mut all = [0; 4];
    assert_eq!(crate::read(alias, &mut all), Ok(4));
    assert_eq!(&all, b"ABBC");
    crate::close(alias).unwrap();
    drop(shared);
    assert!(!path.exists());
}

#[test]
fn native_pins_share_one_capability_and_survive_final_close_and_slot_reuse() {
    use std::os::windows::io::AsRawHandle;
    let path = std::env::temp_dir().join(format!(
        "kinakaze-ofd-pin-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, b"original").unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(&path)
        .unwrap();
    let alias_file = file.try_clone().unwrap();
    let flags = FdFlags::OVERLAPPED
        .union(FdFlags::SEEKABLE)
        .union(FdFlags::READ_ACCESS)
        .union(FdFlags::WRITE_ACCESS);
    let fd = crate::install(file.into_raw_handle() as usize, FdKind::File, flags).unwrap();
    let alias = crate::install_duplicate(
        alias_file.into_raw_handle() as usize,
        FdKind::File,
        flags,
        crate::get(fd).unwrap(),
    )
    .unwrap();
    let (entry, first) = crate::native_pin::pin_native_fd(fd, |_| Ok(())).unwrap();
    let (_, second) = crate::native_pin::pin_native_fd(alias, |_| Ok(())).unwrap();
    if native_pin_cache_enabled() {
        assert_eq!(first.as_raw_handle(), second.as_raw_handle());
    }
    let renamed = path.with_extension("old");
    std::fs::rename(&path, &renamed).unwrap();
    std::fs::write(&path, b"replacement").unwrap();
    crate::close(fd).unwrap();
    crate::close(alias).unwrap();
    let replacement_file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(&path)
        .unwrap();
    let reused = crate::install(
        replacement_file.into_raw_handle() as usize,
        FdKind::File,
        flags,
    )
    .unwrap();
    assert_eq!(reused, fd);
    let mut bytes = [0u8; 8];
    assert_eq!(
        unsafe { first.read_once(entry, bytes.as_mut_ptr(), bytes.len()) },
        Ok(8)
    );
    assert_eq!(&bytes, b"original");
    assert!(
        !descriptions()
            .lock()
            .unwrap()
            .contains_key(&entry.description_id)
    );
    crate::close(reused).unwrap();
    drop(second);
    drop(first);
    drop(
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&renamed)
            .unwrap(),
    );
    std::fs::remove_file(renamed).unwrap();
    std::fs::remove_file(path).unwrap();
}
