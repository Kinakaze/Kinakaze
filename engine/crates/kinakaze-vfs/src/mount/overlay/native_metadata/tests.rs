use super::*;
use windows_sys::Win32::Storage::FileSystem::FILE_WRITE_EA;

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kinakaze-native-metadata-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("jail/sub")).unwrap();
        Self(root)
    }
    fn open(&self, name: &str, follow: bool) -> Option<Object> {
        open(
            &self.0,
            "/jail",
            name,
            follow,
            windows_sys::Win32::Storage::FileSystem::FILE_WRITE_ATTRIBUTES | FILE_WRITE_EA,
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn mutation_targets_the_retained_inode_after_name_replacement() {
    let fixture = Fixture::new();
    let path = fixture.0.join("jail/payload");
    std::fs::write(&path, b"original").unwrap();
    let opened = fixture.open("/payload", true).unwrap();
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    std::fs::write(&path, b"replacement").unwrap();
    crate::fs::set_mode_object(&opened, 0o640, |record| {
        record.uid = Some(456);
        Ok(())
    })
    .unwrap();
    let selected = crate::fs::inode::read_object(&opened).unwrap();
    let replacement = fixture.open("/payload", true).unwrap();
    let fresh = crate::fs::inode::read_object(&replacement).unwrap();
    assert_eq!(selected.mode.unwrap() & 0o7777, 0o640);
    assert_eq!(selected.uid, Some(456));
    assert_ne!(fresh.uid, selected.uid);
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    std::fs::remove_file(path.with_extension("old")).unwrap();
    crate::fs::set_mode_object(&opened, 0o600, |_| Ok(())).unwrap();
    assert_eq!(
        crate::fs::inode::read_object(&opened)
            .unwrap()
            .mode
            .unwrap()
            & 0o7777,
        0o600
    );
}

#[test]
fn links_and_dot_components_keep_the_ordinary_walker() {
    let fixture = Fixture::new();
    let path = fixture.0.join("jail/payload");
    std::fs::write(&path, b"target").unwrap();
    crate::create_emulated_symlink(&fixture.0.join("jail/link"), "payload").unwrap();
    crate::create_emulated_symlink(&fixture.0.join("jail/directory-link"), "sub").unwrap();
    std::fs::write(fixture.0.join("jail/sub/child"), b"child").unwrap();
    assert!(fixture.open("/link", true).is_none());
    let link = fixture.open("/link", false).unwrap();
    assert_eq!(
        crate::fs::inode::read_object(&link)
            .unwrap()
            .symlink
            .as_deref(),
        Some("payload")
    );
    for name in [
        "/directory-link/child",
        "/sub/../payload",
        "/./payload",
        "/payload/",
        "/missing",
    ] {
        assert!(fixture.open(name, true).is_none(), "{name}");
    }
    assert!(fixture.open("/sub", true).is_some());
}

#[test]
fn namespace_prefix_and_linux_name_escaping_select_the_exact_leaf() {
    let fixture = Fixture::new();
    let name = "package:amd64";
    let escaped = crate::path::escape_component(name);
    std::fs::write(fixture.0.join(escaped.as_ref()), b"outside").unwrap();
    std::fs::write(fixture.0.join("jail").join(escaped.as_ref()), b"inside").unwrap();
    let opened = fixture.open(&format!("/{name}"), true).unwrap();
    crate::fs::set_mode_object(&opened, 0o620, |record| {
        record.gid = Some(123);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        crate::fs::inode::read_object(&opened).unwrap().gid,
        Some(123)
    );
    let outside = Object::open(
        &fixture.0.join(escaped.as_ref()),
        FILE_READ_ATTRIBUTES | FILE_READ_EA,
    )
    .unwrap();
    assert_ne!(
        crate::fs::inode::read_object(&outside).unwrap().gid,
        Some(123)
    );
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
    assert!(fixture.open("/sub/payload", true).is_some());
    assert!(fixture.open("/junction/payload", true).is_none());
    assert!(fixture.open("/junction/payload", false).is_none());
    assert!(fixture.open("/junction", false).is_none());
    std::fs::remove_dir(link).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"original");
}
