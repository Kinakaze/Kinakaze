use super::*;
use crate::{FdKind, fs};
use std::os::windows::{fs::OpenOptionsExt, io::IntoRawHandle};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE,
};

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
