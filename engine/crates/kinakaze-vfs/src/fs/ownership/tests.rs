use super::*;
use crate::xattr::Attributes;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};

struct Fixture {
    directory: PathBuf,
    object: Object,
}

impl Fixture {
    fn new(mode: u32) -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("kinakaze-ownership-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("original");
        std::fs::write(&path, b"retained contents").unwrap();
        let object = Object::open(&path, GENERIC_READ | GENERIC_WRITE).unwrap();
        let record = inode::Record {
            mode: Some(S_IFREG | mode),
            uid: Some(1234),
            gid: Some(5678),
            ..inode::Record::default()
        };
        ea::write(&object, inode::EA_NAME, &record.encode().unwrap()).unwrap();
        Self { directory, object }
    }

    fn owner(&self) -> Ownership {
        Ownership {
            uid: 1234,
            gid: 5678,
            caller: 0,
            group_member: true,
        }
    }

    fn attributes(&self) -> Attributes {
        unsafe { Attributes::from_handle(self.object.raw(), true).unwrap() }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn descriptor_entry_checks_flags_and_reads_current_metadata() {
    let fixture = Fixture::new(0o640);
    let object = Object::duplicate(fixture.object.raw()).unwrap();
    let descriptor = crate::install(
        object.into_raw() as usize,
        crate::FdKind::File,
        crate::FdFlags::READ_ACCESS.union(crate::FdFlags::WRITE_ACCESS),
    )
    .unwrap();
    assert_eq!(
        unchanged_descriptor(descriptor, false, &fixture.owner()),
        Ok(true)
    );
    super::super::fchown(descriptor, false, &fixture.owner()).unwrap();
    fixture
        .attributes()
        .set(b"security.capability", b"privilege", 0)
        .unwrap();
    assert_eq!(
        unchanged_descriptor(descriptor, false, &fixture.owner()),
        Ok(false)
    );
    super::super::fchown(descriptor, false, &fixture.owner()).unwrap();
    assert_eq!(
        fixture.attributes().get(b"security.capability"),
        Err(crate::xattr::ENODATA)
    );
    crate::close(descriptor).unwrap();
    let object = Object::duplicate(fixture.object.raw()).unwrap();
    let descriptor = crate::install(
        object.into_raw() as usize,
        crate::FdKind::File,
        crate::FdFlags::PATH_ONLY,
    )
    .unwrap();
    assert_eq!(
        unchanged_descriptor(descriptor, false, &fixture.owner()),
        Err(crate::EBADF)
    );
    assert_eq!(
        unchanged_descriptor(descriptor, true, &fixture.owner()),
        Ok(true)
    );
    crate::close(descriptor).unwrap();
}

#[test]
fn unchanged_descriptor_rejects_a_readonly_mount() {
    let fixture = Fixture::new(0o640);
    let object = Object::duplicate(fixture.object.raw()).unwrap();
    let descriptor = crate::install(
        object.into_raw() as usize,
        crate::FdKind::File,
        crate::FdFlags::READ_ACCESS,
    )
    .unwrap();
    let namespace = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let policy = crate::mount::policy::get(namespace, 42, 1).unwrap();
    crate::mount::native::register(
        crate::get(descriptor).unwrap(),
        crate::mount::native::Description {
            policy,
            path: "/ownership-readonly".into(),
            writer: None,
        },
    )
    .unwrap();
    assert_eq!(
        unchanged_descriptor(descriptor, false, &fixture.owner()),
        Err(crate::EROFS)
    );
    assert_eq!(
        super::super::fchown(descriptor, false, &fixture.owner()),
        Err(crate::EROFS)
    );
    crate::close(descriptor).unwrap();
}

#[test]
fn unchanged_ownership_checks_authorization_and_privilege_effects() {
    let fixture = Fixture::new(0o640);
    let handle = fixture.object.raw();
    let mut owner = fixture.owner();
    assert_eq!(unchanged(handle, &owner), Ok(true));
    owner.caller = 2222;
    assert_eq!(unchanged(handle, &owner), Err(crate::EPERM));
    owner.caller = 1234;
    assert_eq!(unchanged(handle, &owner), Ok(true));
    owner.gid = 9999;
    owner.group_member = false;
    assert_eq!(unchanged(handle, &owner), Err(crate::EPERM));
    owner.group_member = true;
    assert_eq!(unchanged(handle, &owner), Ok(false));
    owner.caller = 0;
    super::super::set_ownership_handle(handle, &owner).unwrap();
    assert_eq!(inode::read(handle).unwrap().gid, Some(9999));
    assert_eq!(unchanged(handle, &owner), Ok(true));
}

#[test]
fn same_owner_still_clears_setid_and_capabilities() {
    for (mode, expected) in [(0o6750, 0o750), (0o2640, 0o2640), (0o4640, 0o640)] {
        let fixture = Fixture::new(mode);
        let handle = fixture.object.raw();
        let attributes = fixture.attributes();
        attributes
            .set(b"security.capability", b"capability", 0)
            .unwrap();
        attributes.set(b"user.retained", b"value", 0).unwrap();
        let owner = fixture.owner();
        assert_eq!(unchanged(handle, &owner), Ok(false));
        super::super::set_ownership_handle(handle, &owner).unwrap();
        assert_eq!(inode::read(handle).unwrap().mode, Some(S_IFREG | expected));
        assert_eq!(
            attributes.get(b"security.capability"),
            Err(crate::xattr::ENODATA)
        );
        assert_eq!(attributes.get(b"user.retained").unwrap(), b"value");
        assert_eq!(unchanged(handle, &owner), Ok(true));
        attributes.set(b"security.capability", b"", 0).unwrap();
        let unspecified = Ownership {
            uid: u32::MAX,
            gid: u32::MAX,
            ..owner
        };
        assert_eq!(unchanged(handle, &unspecified), Ok(true));
        super::super::set_ownership_handle(handle, &unspecified).unwrap();
        assert_eq!(attributes.get(b"security.capability").unwrap(), b"");
        assert_eq!(unchanged(handle, &fixture.owner()), Ok(false));
        super::super::set_ownership_handle(handle, &fixture.owner()).unwrap();
        assert_eq!(
            attributes.get(b"security.capability"),
            Err(crate::xattr::ENODATA)
        );
    }
}

#[test]
fn restricted_handles_reopen_and_incomplete_records_use_full_metadata() {
    let fixture = Fixture::new(0o640);
    for access in [GENERIC_WRITE, GENERIC_READ, FILE_READ_ATTRIBUTES] {
        let handle = Object::open(&fixture.directory.join("original"), access).unwrap();
        assert_eq!(unchanged(handle.raw(), &fixture.owner()), Ok(false));
        super::super::set_ownership_handle(handle.raw(), &fixture.owner()).unwrap();
    }
    let mut record = inode::read(fixture.object.raw()).unwrap();
    record.uid = None;
    ea::write(&fixture.object, inode::EA_NAME, &record.encode().unwrap()).unwrap();
    assert_eq!(unchanged(fixture.object.raw(), &fixture.owner()), Ok(false));
    super::super::set_ownership_handle(fixture.object.raw(), &fixture.owner()).unwrap();
    assert_eq!(inode::read(fixture.object.raw()).unwrap().uid, Some(1234));
}

#[test]
fn shared_queries_retain_inode_after_rename_unlink_and_close() {
    use std::io::{Seek, SeekFrom};
    use std::os::windows::io::AsRawHandle;
    let fixture = Fixture::new(0o640);
    let original = fixture.directory.join("original");
    let moved = fixture.directory.join("moved");
    let mut data = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&original)
        .unwrap();
    data.seek(SeekFrom::Start(7)).unwrap();
    let pinned = Object::duplicate(data.as_raw_handle()).unwrap();
    assert_eq!(unchanged(pinned.raw(), &fixture.owner()), Ok(true));
    assert_eq!(data.stream_position().unwrap(), 7);
    drop(data);
    std::fs::rename(&original, &moved).unwrap();
    std::fs::write(&original, b"replacement").unwrap();
    std::fs::remove_file(&moved).unwrap();
    assert_eq!(unchanged(pinned.raw(), &fixture.owner()), Ok(true));
    assert_eq!(inode::read(pinned.raw()).unwrap().uid, Some(1234));
    assert_eq!(std::fs::read(&original).unwrap(), b"replacement");
}
