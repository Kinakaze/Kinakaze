use super::*;

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("kinakaze-install-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn guest(&self, name: &str) -> String {
        crate::to_guest_path(&self.0.join(name))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn exclusive_install_create_rejects_existing_special_inodes() {
    let f = Fixture::new();
    let flags = O_CREAT | O_EXCL | O_WRONLY;
    let file = f.guest("payload");
    let fd = open(&file, flags, 0o640).unwrap();
    crate::write(fd, b"payload").unwrap();
    crate::close(fd).unwrap();
    assert_eq!(stat(&file).unwrap().st_mode & 0o7777, 0o640);
    create_fifo(&f.guest("fifo"), 0o600).unwrap();
    crate::create_emulated_symlink(&f.0.join("link"), "missing").unwrap();
    for name in ["payload", "fifo", "link"] {
        assert_eq!(open(&f.guest(name), flags, 0o600), Err(crate::EEXIST));
    }
    assert_eq!(std::fs::read(f.0.join("payload")).unwrap(), b"payload");
    assert!(!f.0.join("missing").exists());
}

#[test]
fn install_ownership_retains_inode_identity_and_clears_privilege_bits() {
    let f = Fixture::new();
    let path = f.guest("payload");
    let fd = open(&path, O_CREAT | O_EXCL | O_RDWR, 0o6750).unwrap();
    crate::write(fd, b"payload").unwrap();
    chmod_descriptor(fd, 0o6750, false).unwrap();
    assert_eq!(fstat(fd).unwrap().st_mode & 0o7777, 0o6750);
    std::fs::hard_link(f.0.join("payload"), f.0.join("alias")).unwrap();
    std::fs::rename(f.0.join("payload"), f.0.join("renamed")).unwrap();
    let attrs = crate::xattr::Attributes::from_fd(fd, true).unwrap();
    attrs.set(b"security.capability", b"fixture", 0).unwrap();
    drop(attrs);
    let owner = Ownership {
        uid: 1234,
        gid: 5678,
        caller: 0,
        group_member: true,
    };
    fchown(fd, false, &owner).unwrap();
    for stat in [fstat(fd).unwrap(), stat(&f.guest("alias")).unwrap()] {
        assert_eq!(
            (stat.st_uid, stat.st_gid, stat.st_mode & 0o7777),
            (1234, 5678, 0o750)
        );
        assert_eq!(stat.st_size, 7);
    }
    assert_eq!(
        crate::xattr::Attributes::from_fd(fd, false)
            .unwrap()
            .get(b"security.capability"),
        Err(crate::xattr::ENODATA)
    );
    let denied = Ownership {
        uid: 999,
        gid: u32::MAX,
        caller: 456,
        group_member: false,
    };
    assert_eq!(fchown(fd, false, &denied), Err(crate::EPERM));
    assert_eq!(chown(&f.guest("alias"), true, &denied), Err(crate::EPERM));
    assert_eq!(fstat(fd).unwrap().st_uid, 1234);
    crate::close(fd).unwrap();
    assert_eq!(fchown(-1, false, &owner), Err(crate::EBADF));
    assert_eq!(chown(&f.guest("absent"), true, &owner), Err(crate::ENOENT));
    assert_eq!(
        chown("/proc/kinakaze-missing-chown-target", true, &owner),
        Err(crate::ENOENT)
    );
}
