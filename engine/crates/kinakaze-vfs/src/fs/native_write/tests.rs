use super::*;
use crate::fs::{self, object::Object};

struct Fixture {
    path: std::path::PathBuf,
    previous: crate::fs_context::State,
}
impl Fixture {
    fn new() -> Self {
        crate::fs_context::unshare();
        let previous = crate::fs_context::read(Clone::clone);
        let path = crate::path::default_system_root().join(format!(
            "native-write-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(path.join("jail/sub")).unwrap();
        std::fs::write(path.join("jail/payload"), b"original").unwrap();
        std::fs::write(path.join("payload"), b"outside").unwrap();
        crate::fs_context::update(|state| {
            state.root = Some(path.join("jail"));
            state.root_object = None;
            state.overlay = None;
            state.confined = true;
        });
        Self { path, previous }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        crate::fs_context::update(|state| *state = self.previous.clone());
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}
struct Fd(i32);
impl Drop for Fd {
    fn drop(&mut self) {
        crate::close(self.0).unwrap();
    }
}

#[test]
fn write_open_installs_access_flags_and_retains_inode_across_name_replacement() {
    let fixture = Fixture::new();
    let fd = Fd(
        try_open("//payload", O_WRONLY | O_NONBLOCK | O_CLOEXEC | O_NOFOLLOW)
            .unwrap()
            .unwrap(),
    );
    let entry = crate::get(fd.0).unwrap();
    assert_eq!(entry.kind, FdKind::File);
    for flag in [
        FdFlags::WRITE_ACCESS,
        FdFlags::VERITY_WRITABLE,
        FdFlags::NONBLOCK,
        FdFlags::CLOSE_ON_EXEC,
        FdFlags::OVERLAPPED,
        FdFlags::SEEKABLE,
    ] {
        assert!(entry.flags.contains(flag));
    }
    assert!(!entry.flags.contains(FdFlags::READ_ACCESS));
    let description = crate::mount::native::reference(entry).unwrap().unwrap();
    assert!(description.writer.is_some());
    assert!(description.path.ends_with("/jail/payload"));
    assert_eq!(crate::read(fd.0, &mut [0; 8]), Err(crate::EBADF));
    let reader = Fd(fs::open("/payload", fs::O_RDONLY, 0).unwrap());
    let alias =
        Fd(crate::duplicate_descriptor(fd.0, crate::DuplicateTarget::Lowest, false).unwrap());
    drop(fd);
    std::fs::rename(
        fixture.path.join("jail/payload"),
        fixture.path.join("jail/old"),
    )
    .unwrap();
    std::fs::remove_file(fixture.path.join("jail/old")).unwrap();
    std::fs::write(fixture.path.join("jail/payload"), b"replacement").unwrap();
    assert_eq!(crate::write(alias.0, b"updated!"), Ok(8));
    let mut buffer = [0; 8];
    assert_eq!(crate::read(reader.0, &mut buffer), Ok(8));
    assert_eq!(&buffer, b"updated!");
    assert_eq!(
        std::fs::read(fixture.path.join("jail/payload")).unwrap(),
        b"replacement"
    );
    assert_eq!(
        std::fs::read(fixture.path.join("payload")).unwrap(),
        b"outside"
    );
}

#[test]
fn read_write_open_supports_both_directions_and_blocks_verity_until_closed() {
    let _fixture = Fixture::new();
    let writer = Fd(try_open("/payload", O_RDWR).unwrap().unwrap());
    let entry = crate::get(writer.0).unwrap();
    assert!(entry.flags.contains(FdFlags::READ_ACCESS));
    let mut buffer = [0; 3];
    assert_eq!(crate::read(writer.0, &mut buffer), Ok(3));
    assert_eq!(&buffer, b"ori");
    assert_eq!(crate::write(writer.0, b"after"), Ok(5));
    let reader = Fd(fs::open("/payload", fs::O_RDONLY, 0).unwrap());
    assert_eq!(
        fs::verity::enable(reader.0, 1, 4096, b""),
        Err(26) // ETXTBSY: the retained writable native handle excludes enable.
    );
    drop(writer);
    fs::verity::enable(reader.0, 1, 4096, b"").unwrap();
    assert_eq!(try_open("/payload", O_WRONLY), Err(crate::EPERM));
    assert_eq!(try_open("/payload", O_RDWR), Err(crate::EPERM));
    let mut contents = [0; 8];
    assert_eq!(crate::read(reader.0, &mut contents), Ok(8));
    assert_eq!(&contents, b"oriafter");
}

#[test]
fn write_lookup_rejects_hosted_links_directories_and_special_inodes() {
    let fixture = Fixture::new();
    crate::create_emulated_symlink(&fixture.path.join("jail/link"), "payload").unwrap();
    crate::create_emulated_symlink(&fixture.path.join("jail/directory-link"), "sub").unwrap();
    std::fs::write(fixture.path.join("jail/sub/child"), b"child").unwrap();
    for path in [
        "/link",
        "/directory-link/child",
        "/sub",
        "/missing",
        "/payload/",
        "/sub/../payload",
    ] {
        assert_eq!(try_open(path, O_WRONLY), Ok(None), "{path}");
    }
    for kind in [fs::S_IFIFO, fs::S_IFCHR, fs::S_IFBLK, fs::S_IFSOCK] {
        let path = fixture.path.join("jail/special");
        std::fs::write(&path, b"").unwrap();
        let metadata = Object::open(
            &path,
            FILE_READ_ATTRIBUTES
                | FILE_READ_EA
                | windows_sys::Win32::Storage::FileSystem::FILE_WRITE_EA,
        )
        .unwrap();
        fs::inode::update(metadata.raw(), |record| {
            record.mode = Some(kind | 0o600);
            Ok(())
        })
        .unwrap();
        assert_eq!(try_open("/special", O_WRONLY), Ok(None));
    }
}

#[test]
fn creation_truncation_append_and_complex_names_keep_full_resolution() {
    let fixture = Fixture::new();
    for flag in [
        fs::O_CREAT,
        fs::O_EXCL,
        fs::O_TRUNC,
        fs::O_APPEND,
        fs::O_PATH,
        fs::O_DIRECTORY,
        fs::O_NOATIME,
        0o20000000,
        i32::MIN,
    ] {
        assert_eq!(try_open("/payload", O_WRONLY | flag), Ok(None), "{flag:o}");
    }
    for path in [
        "",
        "/",
        "payload",
        "./payload",
        "/./payload",
        "/payload/",
        "/a\0b",
    ] {
        assert_eq!(try_open(path, O_WRONLY), Ok(None), "{path:?}");
    }
    assert_eq!(try_open("/payload", fs::O_RDONLY), Ok(None));
    assert_eq!(try_open("/payload", 3), Ok(None));
    assert_eq!(
        std::fs::read(fixture.path.join("jail/payload")).unwrap(),
        b"original"
    );
}
