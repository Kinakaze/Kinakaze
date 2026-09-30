use super::*;

#[test]
fn exclusive_create_publishes_complete_metadata_and_preserves_existing_inode() {
    let path = std::env::temp_dir().join(format!(
        "kinakaze-atomic-create-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(path.clone());
    let parent = object::Object::open(&path, FILE_READ_ATTRIBUTES).unwrap();
    for mode in [0o444, 0o640, 0o6750] {
        let name = mode.to_string();
        let record = inode::Record {
            mode: Some(S_IFREG | mode),
            uid: Some(123),
            gid: Some(456),
            ..Default::default()
        };
        let file = parent
            .create_regular_child_with_inode(
                OsStr::new(&name),
                GENERIC_READ | GENERIC_WRITE,
                &record,
            )
            .unwrap();
        // An independent open sees the complete record immediately at publish.
        let observed = inode::read_path(&path.join(&name)).unwrap();
        assert_eq!(
            (observed.mode, observed.uid, observed.gid),
            (record.mode, record.uid, record.gid)
        );
        assert_eq!(
            std::fs::metadata(path.join(&name))
                .unwrap()
                .permissions()
                .readonly(),
            mode & 0o200 == 0
        );
        file.write_at(0, b"created").unwrap();
        assert_eq!(
            parent
                .create_regular_child_with_inode(
                    OsStr::new(&name),
                    GENERIC_WRITE,
                    &Default::default()
                )
                .unwrap_err(),
            crate::EEXIST
        );
        assert_eq!(std::fs::read(path.join(&name)).unwrap(), b"created");
        set_mode_handle(file.raw(), 0o600).unwrap();
    }
    // Exercise the guest O_EXCL route as well as the native creation primitive.
    let guest = crate::to_guest_path(&path.join("guest"));
    let fd = open(&guest, O_CREAT | O_EXCL | O_WRONLY | O_TRUNC, 0o444).unwrap();
    crate::write(fd, b"still writable on the creating descriptor").unwrap();
    assert_eq!(fstat(fd).unwrap().st_mode & 0o7777, 0o444);
    crate::close(fd).unwrap();
    assert_eq!(
        open(&guest, O_CREAT | O_EXCL | O_RDWR, 0o600),
        Err(crate::EEXIST)
    );
    set_mode(&guest, 0o600).unwrap();

    let fd = open(&guest, O_RDWR, 0).unwrap();
    let attrs = crate::xattr::Attributes::from_fd(fd, true).unwrap();
    let mut expected = Vec::new();
    for size in [0, 31, 4096, 48 * 1024] {
        let name = format!("user.payload{size}").into_bytes();
        let bytes = vec![0x9d; size];
        attrs.set(&name, &bytes, 0).unwrap();
        expected.push((name, bytes));
    }
    attrs.set(b"security.capability", b"fixture", 0).unwrap();
    let owner = Ownership {
        uid: 321,
        gid: 654,
        caller: 0,
        group_member: true,
    };
    for _ in 0..16 {
        fchown(fd, false, &owner).unwrap();
    }
    assert_eq!(
        attrs.get(b"security.capability"),
        Err(crate::xattr::ENODATA)
    );
    for (name, bytes) in expected {
        assert_eq!(attrs.get(&name).unwrap(), bytes);
    }
    assert_eq!(fstat(fd).unwrap().st_uid, 321);
    drop(attrs);
    crate::close(fd).unwrap();
}
