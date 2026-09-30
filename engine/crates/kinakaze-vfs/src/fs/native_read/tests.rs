use super::*;
use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_WRITE_EA};

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kinakaze-native-read-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("jail/sub")).unwrap();
        Self(root)
    }
    fn open(&self, path: &str) -> Option<Object> {
        open(&self.0, "/jail", path).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn bytes(object: &Object) -> Vec<u8> {
    let mut buffer = [0; 64];
    let count = object.read_at(0, &mut buffer).unwrap();
    buffer[..count].to_vec()
}

#[test]
fn retained_read_inode_survives_name_replacement_and_unlink() {
    let fixture = Fixture::new();
    let path = fixture.0.join("jail/payload");
    let old = fixture.0.join("jail/old");
    std::fs::write(&path, b"original").unwrap();
    let opened = fixture.open("/payload").unwrap();
    std::fs::rename(&path, &old).unwrap();
    std::fs::write(&path, b"replacement").unwrap();
    std::fs::remove_file(&old).unwrap();
    assert_eq!(bytes(&opened), b"original");
    assert_eq!(bytes(&fixture.open("/payload").unwrap()), b"replacement");
}

#[test]
fn complete_fast_path_installs_the_native_description_and_flags() {
    struct Restore(crate::fs_context::State);
    impl Drop for Restore {
        fn drop(&mut self) {
            crate::fs_context::update(|state| *state = self.0.clone());
        }
    }
    crate::fs_context::unshare();
    let _restore = Restore(crate::fs_context::read(Clone::clone));
    let unique = format!(
        "native-read-install-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let fixture = Fixture(crate::path::default_system_root().join(unique));
    std::fs::create_dir_all(fixture.0.join("jail")).unwrap();
    std::fs::write(fixture.0.join("jail/payload"), b"original").unwrap();
    crate::fs_context::update(|state| {
        state.root = Some(fixture.0.join("jail"));
        state.root_object = None;
        state.overlay = None;
        state.confined = true;
    });
    let fd = try_open_at(
        super::super::AT_FDCWD,
        "/payload",
        "/payload",
        O_CLOEXEC | O_NONBLOCK | O_NOFOLLOW,
    )
    .unwrap()
    .unwrap();
    let entry = crate::get(fd).unwrap();
    assert_eq!(entry.kind, FdKind::File);
    assert!(entry.flags.contains(FdFlags::CLOSE_ON_EXEC));
    assert!(entry.flags.contains(FdFlags::NONBLOCK));
    assert!(entry.flags.contains(FdFlags::READ_ACCESS));
    assert!(!entry.flags.contains(FdFlags::WRITE_ACCESS));
    let description = crate::mount::native::reference(entry).unwrap().unwrap();
    assert!(description.path.ends_with("/jail/payload"));
    assert!(description.writer.is_none());
    let mut buffer = [0; 8];
    assert_eq!(crate::read(fd, &mut buffer).unwrap(), 8);
    assert_eq!(&buffer, b"original");
    crate::close(fd).unwrap();
}

#[test]
fn namespace_prefix_and_escaped_names_select_the_visible_file() {
    let fixture = Fixture::new();
    let name = crate::path::escape_component("name:amd64");
    std::fs::write(fixture.0.join(name.as_ref()), b"outside").unwrap();
    std::fs::write(fixture.0.join("jail").join(name.as_ref()), b"inside").unwrap();
    assert_eq!(bytes(&fixture.open("//name:amd64").unwrap()), b"inside");
}

#[test]
fn cwd_relative_lookup_retains_the_selected_visible_inode() {
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("jail/payload"), b"other directory").unwrap();
    let path = fixture.0.join("jail/sub/payload");
    std::fs::write(&path, b"cwd inode").unwrap();
    let selected = candidate_path(super::super::AT_FDCWD, "payload", "/sub/payload", 0).unwrap();
    let object = fixture.open(selected).unwrap();
    std::fs::rename(&path, fixture.0.join("jail/sub/moved")).unwrap();
    std::fs::write(&path, b"replacement").unwrap();
    assert_eq!(bytes(&object), b"cwd inode");
    assert_eq!(bytes(&fixture.open(selected).unwrap()), b"replacement");
}

#[test]
fn normalized_names_cannot_replace_dirfd_dot_or_trailing_slash_semantics() {
    for original in ["", ".", "..", "./payload", "sub/../payload", "payload/"] {
        assert!(
            candidate_path(super::super::AT_FDCWD, original, "/sub/payload", 0).is_none(),
            "{original:?}"
        );
    }
    assert!(candidate_path(42, "payload", "/sub/payload", 0).is_none());
    assert_eq!(
        candidate_path(42, "/sub/payload", "/sub/payload", 0),
        Some("/sub/payload")
    );
}

#[test]
fn special_inodes_links_and_component_rules_keep_the_walker() {
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("jail/payload"), b"data").unwrap();
    std::fs::write(fixture.0.join("jail/sub/child"), b"child").unwrap();
    crate::create_emulated_symlink(&fixture.0.join("jail/link"), "payload").unwrap();
    crate::create_emulated_symlink(&fixture.0.join("jail/directory-link"), "sub").unwrap();
    for path in [
        "/link",
        "/directory-link/child",
        "/sub/../payload",
        "/./payload",
        "/payload/",
        "/sub",
        "/missing",
        "payload",
    ] {
        assert!(fixture.open(path).is_none(), "{path}");
    }
    for kind in [
        crate::fs::S_IFIFO,
        crate::fs::S_IFCHR,
        crate::fs::S_IFBLK,
        crate::fs::S_IFSOCK,
    ] {
        let path = fixture.0.join("jail/special");
        std::fs::write(&path, b"").unwrap();
        let metadata =
            Object::open(&path, FILE_READ_ATTRIBUTES | FILE_READ_EA | FILE_WRITE_EA).unwrap();
        inode::update(metadata.raw(), |record| {
            record.mode = Some(kind | 0o600);
            Ok(())
        })
        .unwrap();
        assert!(fixture.open("/special").is_none(), "{kind:o}");
        drop(metadata);
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn write_create_directory_and_path_flags_are_ineligible() {
    assert!(eligible(
        "/payload",
        O_CLOEXEC | O_NONBLOCK | O_NOFOLLOW | 0o100000
    ));
    for flags in [
        crate::fs::O_WRONLY,
        crate::fs::O_RDWR,
        crate::fs::O_CREAT,
        crate::fs::O_TRUNC,
        crate::fs::O_APPEND,
        crate::fs::O_PATH,
        crate::fs::O_DIRECTORY,
        crate::fs::O_NOATIME,
        0o20000000,
        i32::MIN,
    ] {
        assert!(!eligible("/payload", flags), "{flags:o}");
    }
    for path in [
        "",
        "/",
        "/payload/",
        "payload",
        "/./payload",
        "/sub/../payload",
        "/a\0b",
    ] {
        assert!(!eligible(path, 0), "{path:?}");
    }
}

#[test]
fn native_junctions_in_ancestors_cannot_bypass_the_guest_walker() {
    use windows_sys::Win32::{
        Foundation::GENERIC_WRITE,
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::IO::DeviceIoControl,
    };
    let fixture = Fixture::new();
    let path = fixture.0.join("jail/sub/payload");
    std::fs::write(&path, b"original").unwrap();
    let link = fixture.0.join("jail/junction");
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
    let target = format!(r"\??\{}", fixture.0.join("jail").join("sub").display());
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
    assert_eq!(std::fs::read(link.join("payload")).unwrap(), b"original");
    assert!(fixture.open("/sub/payload").is_some());
    assert!(fixture.open("/junction/payload").is_none());
    assert!(fixture.open("/junction").is_none());
    std::fs::remove_dir(link).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"original");
}
