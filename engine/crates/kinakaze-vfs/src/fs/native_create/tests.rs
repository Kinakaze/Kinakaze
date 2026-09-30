use super::*;
use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_WRITE_EA};

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "kinakaze-native-create-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(path.join("parent")).unwrap();
        Self(path)
    }
    fn parent(&self) -> PreparedParent {
        PreparedParent::open(&self.0.join("parent"))
            .unwrap()
            .unwrap()
    }
    fn set_parent(&self, mode: u32, gid: u32) {
        let object = Object::open(
            &self.0.join("parent"),
            FILE_READ_ATTRIBUTES | FILE_READ_EA | FILE_WRITE_EA,
        )
        .unwrap();
        inode::update(object.raw(), |record| {
            record.mode = Some(S_IFDIR | mode);
            record.gid = Some(gid);
            Ok(())
        })
        .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn exclusive_create_publishes_live_setgid_metadata_and_retains_parent_identity() {
    let fixture = Fixture::new();
    fixture.set_parent(0o2755, 45678);
    let parent = fixture.parent();
    std::fs::rename(fixture.0.join("parent"), fixture.0.join("moved")).unwrap();
    std::fs::create_dir(fixture.0.join("parent")).unwrap();
    let created = parent
        .create(OsStr::new("payload"), GENERIC_READ | GENERIC_WRITE, 0o640)
        .unwrap();
    assert!(fixture.0.join("moved/payload").exists());
    assert!(!fixture.0.join("parent/payload").exists());
    let record = inode::read(created.raw()).unwrap();
    assert_eq!(
        (record.mode, record.gid),
        (Some(S_IFREG | 0o640), Some(45678))
    );
    assert_eq!(record.uid, Some(crate::credentials::filesystem().uid));
}

#[test]
fn each_creation_reads_current_parent_metadata_without_reusing_observations() {
    let fixture = Fixture::new();
    for (index, mode, gid) in [(0, 0o2755, 11111), (1, 0o2755, 22222), (2, 0o755, 33333)] {
        fixture.set_parent(mode, gid);
        let parent = fixture.parent();
        let name = format!("payload-{index}");
        let created = parent
            .create(OsStr::new(&name), GENERIC_READ | GENERIC_WRITE, 0o600)
            .unwrap();
        let expected = if mode & 0o2000 != 0 {
            gid
        } else {
            crate::credentials::filesystem().gid
        };
        assert_eq!(inode::read(created.raw()).unwrap().gid, Some(expected));
    }
}

#[test]
fn exclusive_creation_never_mutates_existing_leaf_or_link_targets() {
    let fixture = Fixture::new();
    let path = fixture.0.join("parent/payload");
    std::fs::write(&path, b"retained original").unwrap();
    crate::create_emulated_symlink(&fixture.0.join("parent/link"), "missing").unwrap();
    std::fs::create_dir(fixture.0.join("parent/directory")).unwrap();
    let parent = fixture.parent();
    for name in ["payload", "link", "directory"] {
        assert_eq!(
            parent
                .create(OsStr::new(name), GENERIC_WRITE, 0o600)
                .unwrap_err(),
            crate::EEXIST,
            "{name}"
        );
    }
    assert_eq!(std::fs::read(path).unwrap(), b"retained original");
    assert!(!fixture.0.join("parent/missing").exists());
}

#[test]
fn hosted_parent_links_and_regular_parents_keep_the_walker() {
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("regular"), b"file").unwrap();
    crate::create_emulated_symlink(&fixture.0.join("link"), "parent").unwrap();
    for name in ["regular", "link", "missing"] {
        assert!(
            PreparedParent::open(&fixture.0.join(name))
                .unwrap()
                .is_none(),
            "{name}"
        );
    }
}

#[test]
fn native_create_installs_access_flags_and_writable_verity_exclusion() {
    struct Restore(crate::fs_context::State);
    impl Drop for Restore {
        fn drop(&mut self) {
            crate::fs_context::update(|state| *state = self.0.clone());
        }
    }
    crate::fs_context::unshare();
    let _restore = Restore(crate::fs_context::read(Clone::clone));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixture = Fixture(crate::path::default_system_root().join(format!(
        "native-create-install-{}-{stamp}",
        std::process::id()
    )));
    std::fs::create_dir_all(fixture.0.join("jail")).unwrap();
    crate::fs_context::update(|state| {
        state.root = Some(fixture.0.join("jail"));
        state.root_object = None;
        state.overlay = None;
        state.confined = true;
    });
    let flags = O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC | O_NONBLOCK | O_APPEND;
    let fd = try_open_at(super::super::AT_FDCWD, "/payload", "/payload", flags, 0o640)
        .unwrap()
        .unwrap();
    let entry = crate::get(fd).unwrap();
    for flag in [
        FdFlags::READ_ACCESS,
        FdFlags::WRITE_ACCESS,
        FdFlags::CLOSE_ON_EXEC,
        FdFlags::NONBLOCK,
        FdFlags::APPEND,
        FdFlags::VERITY_WRITABLE,
    ] {
        assert!(entry.flags.contains(flag));
    }
    crate::write(fd, b"original").unwrap();
    super::super::lseek(fd, 0, 0).unwrap();
    let mut buffer = [0; 8];
    assert_eq!(crate::read(fd, &mut buffer).unwrap(), 8);
    assert_eq!(&buffer, b"original");
    std::fs::rename(fixture.0.join("jail/payload"), fixture.0.join("jail/moved")).unwrap();
    std::fs::write(fixture.0.join("jail/payload"), b"replacement").unwrap();
    super::super::sync_descriptor(fd).unwrap();
    assert_eq!(super::super::fstat(fd).unwrap().st_size, 8);
    crate::close(fd).unwrap();
}

#[test]
fn existing_readonly_and_nonexclusive_opens_remain_ineligible() {
    assert!(eligible_flags(O_CREAT | O_EXCL | O_WRONLY));
    assert!(eligible_flags(
        O_CREAT | O_EXCL | O_RDWR | O_NOFOLLOW | O_TRUNC
    ));
    for flags in [
        0,
        O_CREAT | O_EXCL,
        O_CREAT | O_WRONLY,
        O_EXCL | O_WRONLY,
        O_CREAT | O_EXCL | O_WRONLY | super::super::O_PATH,
        O_CREAT | O_EXCL | O_RDWR | super::super::O_DIRECTORY,
        O_CREAT | O_EXCL | O_WRONLY | super::super::O_NOATIME,
        O_CREAT | O_EXCL | 3,
        i32::MIN,
    ] {
        assert!(!eligible_flags(flags), "{flags:o}");
    }
}

#[test]
fn ancestor_junction_cannot_redirect_exclusive_creation() {
    use windows_sys::Win32::{
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::IO::DeviceIoControl,
    };
    let fixture = Fixture::new();
    let link = fixture.0.join("junction");
    std::fs::create_dir(fixture.0.join("parent/child")).unwrap();
    std::fs::create_dir(&link).unwrap();
    let object = Object::owned(unsafe {
        CreateFileW(
            crate::path::wide_path(&link).unwrap().as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    })
    .unwrap();
    let target = format!(r"\??\{}", fixture.0.join("parent").display());
    let bytes: Vec<u8> = target.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut data = vec![0u8; 16];
    data[..4].copy_from_slice(&0xa000_0003u32.to_le_bytes());
    data[4..6].copy_from_slice(&((12 + bytes.len()) as u16).to_le_bytes());
    data[10..12].copy_from_slice(&(bytes.len() as u16).to_le_bytes());
    data[12..14].copy_from_slice(&((bytes.len() + 2) as u16).to_le_bytes());
    data.extend(bytes);
    data.extend([0; 4]);
    let mut returned = 0;
    assert_ne!(
        unsafe {
            DeviceIoControl(
                object.raw(),
                0x0009_00a4,
                data.as_ptr().cast(),
                data.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        },
        0
    );
    drop(object);
    assert!(
        PreparedParent::open(&fixture.0.join("parent/child"))
            .unwrap()
            .is_some()
    );
    assert!(PreparedParent::open(&link).unwrap().is_none());
    assert!(PreparedParent::open(&link.join("child")).unwrap().is_none());
    assert!(!fixture.0.join("parent/payload").exists());
    std::fs::remove_dir(link).unwrap();
}
