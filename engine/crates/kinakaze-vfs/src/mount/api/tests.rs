use super::*;

#[test]
fn recursive_readonly_failure_preserves_parent_and_child_policies() {
    let mut a = Fixture::new();
    let context = a.context();
    let tree = a.mount(context);
    move_tree(tree, &a.path("mount"), 4).unwrap();
    fs::mkdir(&a.path("mount/child"), 0o755).unwrap();
    let mut b = Fixture::new();
    let context = b.context();
    let child = b.mount(context);
    move_tree(child, &a.path("mount/child"), 4).unwrap();
    let busy = fs::open(&a.path("mount/child/data"), fs::O_WRONLY, 0).unwrap();
    assert_eq!(
        set_attributes(&a.path("mount"), 0x8000, 1, 0, 0, 0),
        Err(EBUSY)
    );
    let parent = fs::open(&a.path("mount/data"), fs::O_WRONLY, 0).unwrap();
    crate::close(parent).unwrap();
    crate::close(busy).unwrap();
    set_attributes(&a.path("mount"), 0x8000, 1, 0, 0, 0).unwrap();
    assert_eq!(
        fs::open(&a.path("mount/child/data"), fs::O_WRONLY, 0),
        Err(crate::EROFS)
    );
    assert_eq!(
        fs::open(&a.path("mount/data"), fs::O_WRONLY, 0),
        Err(crate::EROFS)
    );
    super::super::unmount(&a.path("mount/child"), 2).unwrap();
}

#[test]
fn ownership_copyup_and_unlinked_descriptor_preserve_inode_identity() {
    let mut f = Fixture::new();
    let context = f.context();
    let tree = f.mount(context);
    let path = format!("{}/data", tree_path(tree).unwrap());
    let request = |uid, gid, caller| fs::Ownership {
        uid,
        gid,
        caller,
        group_member: false,
    };
    fs::chown(&path, true, &request(1000, 2000, 0)).unwrap();
    assert_eq!(fs::stat(&path).unwrap().st_uid, 1000);
    assert_eq!(fs::stat(&f.path("lower/data")).unwrap().st_uid, 0);
    assert_eq!(
        fs::chown(&path, true, &request(0, u32::MAX, 1000)),
        Err(crate::EPERM)
    );
    let fd = fs::openat(tree, "data", fs::O_RDONLY, 0).unwrap();
    f.fds.push(fd);
    fs::unlink(&path).unwrap();
    fs::fchown(fd, false, &request(u32::MAX, 3000, 0)).unwrap();
    let stat = fs::fstat(fd).unwrap();
    assert_eq!((stat.st_uid, stat.st_gid), (1000, 3000));
}

#[test]
fn superblock_reconfigure_tracks_all_clones_and_shared_mapping_owners() {
    let mut f = Fixture::new();
    let context = f.context();
    let tree = f.mount(context);
    let root = tree_path(tree).unwrap();
    let clone = open_tree(&root, 1).unwrap();
    f.fds.push(clone);
    let writer = fs::openat(clone, "data", fs::O_RDWR, 0).unwrap();
    let mapping = fs::verity::Opened::from_fd(writer)
        .unwrap()
        .mount_writer()
        .unwrap();
    crate::close(writer).unwrap();
    configure(context, 0, Some("ro"), Some(Parameter::Flag)).unwrap();
    assert_eq!(configure(context, 7, None, None), Err(EBUSY));
    assert!(
        !overlay::mount_metadata(&tree_mount(tree, "/").unwrap().0)
            .unwrap()
            .1
    );
    drop(mapping);
    configure(context, 7, None, None).unwrap();
    assert_eq!(fs::openat(tree, "data", fs::O_WRONLY, 0), Err(crate::EROFS));
    assert_eq!(
        fs::openat(clone, "data", fs::O_WRONLY, 0),
        Err(crate::EROFS)
    );
    configure(context, 0, Some("rw"), Some(Parameter::Flag)).unwrap();
    configure(context, 7, None, None).unwrap();
    let writer = fs::openat(clone, "data", fs::O_WRONLY, 0).unwrap();
    crate::close(writer).unwrap();
}

#[test]
fn directory_magic_links_keep_detached_tree_after_origin_fd_is_closed() {
    let mut f = Fixture::new();
    std::fs::create_dir(f.root.join("lower/sub")).unwrap();
    std::fs::write(f.root.join("lower/sub/item"), b"nested").unwrap();
    let context = f.context();
    let tree = f.mount(context);
    let dir = fs::openat(tree, "sub", fs::O_RDONLY | fs::O_DIRECTORY, 0).unwrap();
    crate::close(tree).unwrap();
    f.fds.retain(|fd| *fd != tree);
    f.fds.push(dir);
    assert_eq!(f.read(dir, "item"), b"nested");
    assert_eq!(f.read(dir, "../data"), b"lower");
    let writer = fs::openat(dir, "new", fs::O_WRONLY | fs::O_CREAT, 0o600).unwrap();
    crate::write(writer, b"created").unwrap();
    crate::close(writer).unwrap();
    assert_eq!(f.read(dir, "new"), b"created");
    assert!(!f.root.join("lower/sub/new").exists());
    let clone = open_tree(&format!("/proc/self/fd/{dir}"), 1).unwrap();
    f.fds.push(clone);
    assert_eq!(f.read(clone, "item"), b"nested");
    set_attributes(&format!("/proc/self/fd/{dir}"), 0, 1, 0, 0, 0).unwrap();
    assert_eq!(fs::openat(dir, "new", fs::O_WRONLY, 0), Err(crate::EROFS));
}

#[test]
fn readonly_transition_tracks_writers_aliases_and_metadata_operations() {
    let mut f = Fixture::new();
    let context = f.context();
    let tree = f.mount(context);
    let path = tree_path(tree).unwrap();
    let writer = fs::openat(tree, "data", fs::O_RDWR, 0).unwrap();
    assert_eq!(set_attributes(&path, 0, 1, 0, 0, 0), Err(EBUSY));
    let mapped = fs::verity::Opened::from_fd(writer)
        .unwrap()
        .mount_writer()
        .unwrap();
    crate::close(writer).unwrap();
    assert_eq!(set_attributes(&path, 0, 1, 0, 0, 0), Err(EBUSY));
    drop(mapped);
    set_attributes(&path, 0, 1, 0, 0, 0).unwrap();
    assert_eq!(fs::openat(tree, "data", fs::O_WRONLY, 0), Err(crate::EROFS));
    let read = fs::openat(tree, "data", fs::O_RDONLY, 0).unwrap();
    f.fds.push(read);
    assert_eq!(overlay::chmod_descriptor(read, 0o600), Err(crate::EROFS));
    set_attributes(&path, 0, 0, 1, 0, 0).unwrap();
    let stamp = overlay::metadata_handle(read, true, false)
        .unwrap()
        .unwrap();
    assert_eq!(set_attributes(&path, 0, 1, 0, 0, 0), Err(EBUSY));
    drop(stamp);
    set_attributes(&path, 0, 1, 0, 0, 0).unwrap();
    set_attributes(&path, 0, 0, 1, 0, 0).unwrap();
    assert_eq!(overlay::chmod_descriptor(read, 0o600), Ok(true));
}

#[test]
fn noexec_nodev_are_enforced_on_detached_and_moved_mounts() {
    let mut f = Fixture::new();
    let context = f.context();
    let tree = f.mount(context);
    let path = tree_path(tree).unwrap();
    fs::create_device(&format!("{path}/null"), fs::S_IFCHR | 0o666, 0x103).unwrap();
    set_attributes(&path, 0, 8 | 4 | 2, 0, 0, 0).unwrap();
    assert_eq!(
        overlay::check_execute(&format!("{path}/data")),
        Err(crate::EACCES)
    );
    assert_eq!(
        fs::openat(tree, "null", fs::O_RDONLY, 0),
        Err(crate::EACCES)
    );
    let device = fs::openat(tree, "null", fs::O_PATH, 0).unwrap();
    f.fds.push(device);
    assert_eq!(
        fs::open(&format!("/proc/self/fd/{device}"), fs::O_RDONLY, 0),
        Err(crate::EACCES)
    );
    let reader = fs::openat(tree, "data", fs::O_RDONLY, 0).unwrap();
    f.fds.push(reader);
    assert!(fs::verity::Opened::from_fd(reader).unwrap().mount_noexec());
    move_tree(tree, &f.path("mount"), 4).unwrap();
    set_attributes(&f.path("mount"), 0, 0, 8 | 4 | 2, 0, 0).unwrap();
    assert!(!fs::verity::Opened::from_fd(reader).unwrap().mount_noexec());
    assert_eq!(overlay::check_execute(&f.path("mount/data")), Ok(()));
    let opened = fs::open(&format!("/proc/self/fd/{device}"), fs::O_RDONLY, 0).unwrap();
    assert_eq!(crate::get(opened).unwrap().kind, FdKind::Null);
    crate::close(opened).unwrap();
}

struct Fixture {
    root: PathBuf,
    fds: Vec<i32>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kinakaze-mount-api-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for part in ["lower", "upper", "work", "mount", "clone", "moved"] {
            std::fs::create_dir_all(root.join(part)).unwrap();
        }
        std::fs::write(root.join("lower/data"), b"lower").unwrap();
        Self {
            root,
            fds: Vec::new(),
        }
    }
    fn path(&self, tail: &str) -> String {
        crate::to_guest_path(&self.root.join(tail))
    }
    fn context(&mut self) -> i32 {
        let fd = fsopen("overlay", 0).unwrap();
        self.fds.push(fd);
        for (key, path) in [
            ("lowerdir+", "lower"),
            ("upperdir", "upper"),
            ("workdir", "work"),
        ] {
            configure(fd, 1, Some(key), Some(Parameter::String(&self.path(path)))).unwrap();
        }
        fd
    }
    fn mount(&mut self, fd: i32) -> i32 {
        configure(fd, 6, None, None).unwrap();
        let tree = fsmount(fd, 0, 0).unwrap();
        self.fds.push(tree);
        tree
    }
    fn read(&mut self, dir: i32, path: &str) -> Vec<u8> {
        let fd = fs::openat(dir, path, fs::O_RDONLY, 0).unwrap();
        self.fds.push(fd);
        let mut b = vec![0; 64];
        let n = crate::read(fd, &mut b).unwrap();
        b.truncate(n);
        b
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for fd in self.fds.drain(..) {
            let _ = crate::close(fd);
        }
        for p in ["moved", "clone", "mount"] {
            let _ = super::super::unmount(&self.path(p), 2);
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn context_errors_are_shared_and_do_not_poison_parameter_state() {
    let mut f = Fixture::new();
    let fd = f.context();
    let dup = shared::object_fd(fd)
        .unwrap()
        .descriptor_kind(
            FdKind::FsContext,
            FdFlags::READ_ACCESS.union(FdFlags::WRITE_ACCESS),
        )
        .unwrap();
    f.fds.push(dup);
    assert_eq!(
        configure(fd, 1, Some("missing"), Some(Parameter::String("on"))),
        Err(EINVAL)
    );
    assert_eq!(crate::read(dup, &mut [0; 1]), Err(EMSGSIZE));
    let mut log = [0; 128];
    let n = crate::read(dup, &mut log).unwrap();
    assert!(std::str::from_utf8(&log[..n]).unwrap().contains("errno 22"));
    assert_eq!(crate::read(fd, &mut log), Err(ENODATA));
    let tree = f.mount(dup);
    assert_eq!(fs::fstat(tree).unwrap().st_mode & fs::S_IFMT, fs::S_IFDIR);
    assert_eq!(fsmount(fd, 0, 0), Err(EBUSY));
}

#[test]
fn pinned_fd_layers_survive_rename_and_path_replacement() {
    let mut f = Fixture::new();
    let fd = fsopen("overlay", 1).unwrap();
    f.fds.push(fd);
    let lower = fs::open(&f.path("lower"), fs::O_PATH | fs::O_DIRECTORY, 0).unwrap();
    configure(fd, 5, Some("lowerdir+"), Some(Parameter::Fd(lower))).unwrap();
    crate::close(lower).unwrap();
    for (key, path) in [("upperdir", "upper"), ("workdir", "work")] {
        configure(fd, 1, Some(key), Some(Parameter::String(&f.path(path)))).unwrap();
    }
    std::fs::rename(f.root.join("lower"), f.root.join("original")).unwrap();
    std::fs::create_dir(f.root.join("lower")).unwrap();
    std::fs::write(f.root.join("lower/data"), b"wrong inode").unwrap();
    let tree = f.mount(fd);
    assert_eq!(f.read(tree, "data"), b"lower");
    let file = fs::openat(tree, "data", fs::O_WRONLY | fs::O_TRUNC, 0).unwrap();
    f.fds.push(file);
    crate::write(file, b"upper").unwrap();
    assert_eq!(
        std::fs::read(f.root.join("original/data")).unwrap(),
        b"lower"
    );
    move_tree(tree, &f.path("mount"), 4).unwrap();
    assert_eq!(f.read(fs::AT_FDCWD, &f.path("mount/data")), b"upper");
}

#[test]
fn detached_clone_survives_unmount_and_moves_without_changing_lower() {
    let mut f = Fixture::new();
    let fd = f.context();
    let tree = f.mount(fd);
    move_tree(tree, &f.path("mount"), 4).unwrap();
    let cloned = open_tree(&f.path("mount"), 1 | 0x80000).unwrap();
    f.fds.push(cloned);
    assert!(
        crate::get(cloned)
            .unwrap()
            .flags
            .contains(FdFlags::CLOSE_ON_EXEC)
    );
    super::super::unmount(&f.path("mount"), 2).unwrap();
    assert_eq!(f.read(cloned, "data"), b"lower");
    move_tree(cloned, &f.path("clone"), 4).unwrap();
    move_path(&f.path("clone"), &f.path("moved"), 0).unwrap();
    assert_eq!(f.read(fs::AT_FDCWD, &f.path("moved/data")), b"lower");
    assert_eq!(fs::stat(&f.path("clone/data")).unwrap_err(), ENOENT);
}

#[test]
fn failed_multilayer_parameter_is_atomic() {
    let mut f = Fixture::new();
    let fd = fsopen("overlay", 0).unwrap();
    f.fds.push(fd);
    let bad = format!(
        "{}:{}",
        escaped(&f.path("lower")),
        escaped(&f.path("missing"))
    );
    assert_eq!(
        configure(fd, 1, Some("lowerdir"), Some(Parameter::String(&bad))),
        Err(ENOENT)
    );
    configure(
        fd,
        1,
        Some("lowerdir+"),
        Some(Parameter::String(&f.path("lower"))),
    )
    .unwrap();
    let tree = f.mount(fd);
    assert_eq!(f.read(tree, "data"), b"lower");
}

#[test]
fn readonly_attachment_on_writable_superblock_denies_copyup_and_metadata() {
    let mut f = Fixture::new();
    let fd = f.context();
    configure(fd, 6, None, None).unwrap();
    let tree = fsmount(fd, 0, 1).unwrap();
    f.fds.push(tree);
    assert_eq!(fs::openat(tree, "data", fs::O_WRONLY, 0), Err(crate::EROFS));
    assert_eq!(
        fs::mkdir(&format!("{}/new", tree_path(tree).unwrap()), 0o755),
        Err(crate::EROFS)
    );
    assert!(overlay::descriptor_filesystem(tree).unwrap().unwrap().2);
    assert_eq!(f.read(tree, "data"), b"lower");
    assert!(!f.root.join("upper/data").exists());
    let file = fs::openat(tree, "data", fs::O_RDONLY, 0).unwrap();
    f.fds.push(file);
    crate::close(tree).unwrap();
    f.fds.retain(|fd| *fd != tree);
    assert_eq!(overlay::chmod_descriptor(file, 0o600), Err(crate::EROFS));
}

#[test]
fn nosymfollow_changes_only_selected_detached_attachment() {
    let mut f = Fixture::new();
    let fd = f.context();
    let tree = f.mount(fd);
    let root = tree_path(tree).unwrap();
    let mut link = overlay::prepare_create(&format!("{root}/link")).unwrap();
    crate::create_emulated_symlink(&link, "data").unwrap();
    link.finish().unwrap();
    drop(link);
    let clone = open_tree(&root, 1 | 0x1000).unwrap();
    f.fds.push(clone);
    set_attributes(&root, 0x1000, 0x200000, 0, 0, 0).unwrap();
    assert_eq!(fs::openat(tree, "link", fs::O_RDONLY, 0), Err(crate::ELOOP));
    assert_eq!(f.read(clone, "link"), b"lower");
    set_attributes(&root, 0x1000, 0, 0x200000, 0, 0).unwrap();
    assert_eq!(f.read(tree, "link"), b"lower");
}

#[test]
fn mount_queries_agree_with_live_attachment_and_page_after_id() {
    let mut f = Fixture::new();
    let fd = f.context();
    let tree = f.mount(fd);
    move_tree(tree, &f.path("mount"), 4).unwrap();
    let id = super::super::query::id_for_path(&f.path("mount")).unwrap();
    assert!(
        super::super::query::list(u64::MAX, 0, 1_000_000, false)
            .unwrap()
            .contains(&id)
    );
    assert!(
        !super::super::query::list(u64::MAX, id, 1_000_000, false)
            .unwrap()
            .contains(&id)
    );
    let bytes = super::super::query::stat_fd(tree, 2 | 16 | 32).unwrap();
    assert_eq!(u64::from_le_bytes(bytes[40..48].try_into().unwrap()), id);
    super::super::unmount(&f.path("mount"), 2).unwrap();
    assert_eq!(super::super::query::stat(id, 2), Err(ENOENT));
    assert!(super::super::query::stat_fd(tree, 2).is_ok());
}

#[test]
fn cloning_through_symlink_preserves_merged_lower_names() {
    let mut f = Fixture::new();
    let context = f.context();
    let tree = f.mount(context);
    move_tree(tree, &f.path("mount"), 4).unwrap();
    crate::create_emulated_symlink(&f.root.join("alias"), &f.path("mount")).unwrap();
    let clone = open_tree(&f.path("alias"), 1).unwrap();
    f.fds.push(clone);
    assert_eq!(f.read(clone, "data"), b"lower");
    assert_eq!(open_tree(&f.path("alias"), 1 | 0x100), Err(EOPNOTSUPP));
    assert_eq!(fspick(&f.path("mount/missing"), 0), Err(ENOENT));
}
