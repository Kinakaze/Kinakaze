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
    let flags = O_CREAT | O_EXCL | O_WRONLY | O_NOFOLLOW;
    let file = f.guest("payload");
    let fd = open(&file, flags, 0o640).unwrap();
    crate::write(fd, b"payload").unwrap();
    crate::close(fd).unwrap();
    assert_eq!(stat(&file).unwrap().st_mode & 0o7777, 0o640);
    create_fifo(&f.guest("fifo"), 0o600).unwrap();
    crate::create_emulated_symlink(&f.0.join("link"), "missing").unwrap();
    std::fs::create_dir(f.0.join("directory")).unwrap();
    for access in [O_RDONLY, O_WRONLY, O_RDWR] {
        for nofollow in [0, O_NOFOLLOW] {
            for name in ["payload", "fifo", "link", "directory"] {
                assert_eq!(
                    open(&f.guest(name), O_CREAT | O_EXCL | access | nofollow, 0o600),
                    Err(crate::EEXIST),
                    "exclusive create: {name}, access={access}, nofollow={nofollow}"
                );
            }
        }
    }
    let directory = open(&f.guest("directory"), O_DIRECTORY | O_RDONLY, 0).unwrap();
    assert_eq!(crate::get(directory).unwrap().kind, FdKind::Directory);
    crate::close(directory).unwrap();
    assert_eq!(std::fs::read(f.0.join("payload")).unwrap(), b"payload");
    assert!(!f.0.join("missing").exists());
    assert_eq!(
        open(&f.guest("link"), O_WRONLY | O_NOFOLLOW, 0),
        Err(crate::ELOOP)
    );
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

#[test]
fn repeated_install_metadata_still_clears_capabilities_and_checks_permissions() {
    let f = Fixture::new();
    let path = f.guest("payload");
    let fd = open(&path, O_CREAT | O_EXCL | O_RDWR, 0o644).unwrap();
    let owner = Ownership {
        uid: 0,
        gid: 0,
        caller: 0,
        group_member: true,
    };
    fchown(fd, false, &owner).unwrap();
    let attrs = crate::xattr::Attributes::from_fd(fd, true).unwrap();
    attrs.set(b"user.retained", b"unchanged", 0).unwrap();
    for _ in 0..3 {
        // UID/GID/mode already match; chown must still remove a newly added
        // capability, and repeated chmod must preserve the other inode fields.
        attrs.set(b"security.capability", b"fixture", 0).unwrap();
        fchown(fd, false, &owner).unwrap();
        assert_eq!(
            attrs.get(b"security.capability"),
            Err(crate::xattr::ENODATA)
        );
        chmod_descriptor(fd, 0o644, false).unwrap();
        let stat = fstat(fd).unwrap();
        assert_eq!(
            (stat.st_uid, stat.st_gid, stat.st_mode),
            (0, 0, S_IFREG | 0o644)
        );
        assert_eq!(attrs.get(b"user.retained").unwrap(), b"unchanged");
    }
    let denied = Ownership {
        caller: 123,
        ..owner
    };
    assert_eq!(fchown(fd, false, &denied), Err(crate::EPERM));
    chmod_descriptor(fd, 0o6750, false).unwrap();
    fchown(fd, false, &owner).unwrap();
    assert_eq!(fstat(fd).unwrap().st_mode & 0o7777, 0o750);
    drop(attrs);
    crate::close(fd).unwrap();
}

#[test]
fn writable_stat_reads_live_inode_metadata_and_survives_unlink() {
    let f = Fixture::new();
    let path = f.guest("live-stat");
    let fd = open(&path, O_CREAT | O_EXCL | O_RDWR, 0o640).unwrap();
    assert!(
        crate::get(fd)
            .unwrap()
            .flags
            .contains(FdFlags::VERITY_WRITABLE)
    );
    crate::write(fd, b"payload").unwrap();
    let original = fstat(fd).unwrap();
    let reader = open(&path, O_RDONLY, 0).unwrap();
    assert_eq!(fstat(reader).unwrap().st_size, 7);
    for (mode, uid) in [(0o600, 123), (0o644, 456)] {
        chmod_descriptor(fd, mode, false).unwrap();
        fchown(
            fd,
            false,
            &Ownership {
                uid,
                gid: uid + 1,
                caller: 0,
                group_member: true,
            },
        )
        .unwrap();
        for stat in [fstat(fd).unwrap(), fstat(reader).unwrap()] {
            assert_eq!(
                (
                    stat.st_mode & 0o7777,
                    stat.st_uid,
                    stat.st_gid,
                    stat.st_size
                ),
                (mode, uid, uid + 1, 7)
            );
            assert_eq!(stat.st_ino, original.st_ino);
        }
    }
    unlink(&path).unwrap();
    assert_eq!(fstat(fd).unwrap().st_size, 7);
    assert_eq!(fstat(reader).unwrap().st_size, 7);
    crate::close(reader).unwrap();
    crate::close(fd).unwrap();
}

#[test]
fn descriptor_chmod_keeps_the_pinned_inode_and_rejects_link_and_stale_slots() {
    let f = Fixture::new();
    let path = f.guest("chmod-pinned");
    let fd = open(&path, O_CREAT | O_EXCL | O_RDWR, 0o640).unwrap();
    crate::write(fd, b"original").unwrap();
    fchown(
        fd,
        false,
        &Ownership {
            uid: 123,
            gid: 456,
            caller: 0,
            group_member: true,
        },
    )
    .unwrap();
    let original = fstat(fd).unwrap();
    unlink(&path).unwrap();
    let replacement = open(&path, O_CREAT | O_EXCL | O_RDWR, 0o644).unwrap();
    let entry = crate::get(fd).unwrap();
    assert!(matches!(
        crate::mount::overlay::chmod_handle(fd, false, entry.generation.wrapping_add(1)),
        Err(EBADF)
    ));
    chmod_descriptor(fd, 0o600, false).unwrap();
    let changed = fstat(fd).unwrap();
    assert_eq!(
        (
            changed.st_ino,
            changed.st_mode & 0o7777,
            changed.st_uid,
            changed.st_gid,
            changed.st_size
        ),
        (original.st_ino, 0o600, 123, 456, 8)
    );
    assert_eq!(fstat(replacement).unwrap().st_mode & 0o7777, 0o644);
    crate::close(replacement).unwrap();
    crate::close(fd).unwrap();

    let link = f.0.join("chmod-link");
    crate::create_emulated_symlink(&link, "chmod-pinned").unwrap();
    let fd = open(&f.guest("chmod-link"), O_PATH | O_NOFOLLOW, 0).unwrap();
    assert_eq!(chmod_descriptor(fd, 0o600, false), Err(EBADF));
    assert_eq!(chmod_descriptor(fd, 0o600, true), Err(crate::EOPNOTSUPP));
    assert_eq!(read_link_fd(fd).unwrap(), "chmod-pinned");
    assert_eq!(stat(&path).unwrap().st_mode & 0o7777, 0o644);
    crate::close(fd).unwrap();
}
