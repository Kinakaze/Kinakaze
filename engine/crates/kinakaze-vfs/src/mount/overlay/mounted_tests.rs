use super::*;

#[test]
fn log_symlinks_cross_overlay_tmpfs_and_proc_to_the_same_pipe() {
    fn symlink(target: &str, link: &str) {
        if crate::tmpfs::create(link, S_IFLNK | 0o777, 0, target).unwrap() {
            return;
        }
        let mut path = prepare_create(link).unwrap();
        crate::create_emulated_symlink(&path, target).unwrap();
        path.initialize_created(S_IFLNK | 0o777).unwrap();
        path.finish().unwrap();
    }
    let f = Fixture::new();
    let proc = f.guest("empty");
    let dev = f.guest("dir");
    crate::mount::proc_mount(&proc, 0, "").unwrap();
    crate::mount::tmpfs_mount(&dev, 0, "mode=755").unwrap();
    let (reader, writer) = crate::create_pipe(crate::FdFlags::NONBLOCK, 4096).unwrap();
    let link = format!("{dev}/stderr");
    symlink(&format!("{proc}/self/fd/{writer}"), &link);
    symlink(&link, &f.guest("error.log"));
    let flags = fs::O_WRONLY | fs::O_APPEND | fs::O_CREAT | fs::O_CLOEXEC;
    let log = fs::open(&f.guest("error.log"), flags, 0o644).unwrap();
    crate::write(log, b"log record\n").unwrap();
    let mut bytes = [0; 11];
    assert_eq!(crate::read(reader, &mut bytes).unwrap(), bytes.len());
    assert_eq!(&bytes, b"log record\n");
    assert_ne!(
        crate::get(log).unwrap().description_id,
        crate::get(writer).unwrap().description_id
    );
    assert!(
        crate::get(log)
            .unwrap()
            .flags
            .contains(crate::FdFlags::CLOSE_ON_EXEC)
    );
    assert!(
        !crate::get(writer)
            .unwrap()
            .flags
            .contains(crate::FdFlags::APPEND)
    );
    assert_eq!(
        fs::open(&f.guest("error.log"), flags | fs::O_NOFOLLOW, 0o644),
        Err(crate::ELOOP)
    );
    assert_eq!(
        fs::open(&f.guest("error.log"), flags | fs::O_EXCL, 0o644),
        Err(crate::EEXIST)
    );
    assert_eq!(
        fs::open(&f.guest("error.log"), fs::O_RDONLY | fs::O_DIRECTORY, 0),
        Err(crate::ENOTDIR)
    );
    // A cycle between two backends must share one symlink limit.
    symlink(&f.guest("cycle"), &format!("{dev}/cycle"));
    symlink(&format!("{dev}/cycle"), &f.guest("cycle"));
    assert_eq!(
        fs::open(&f.guest("cycle"), fs::O_RDONLY, 0),
        Err(crate::ELOOP)
    );
    for fd in [log, writer, reader] {
        crate::close(fd).unwrap();
    }
    crate::mount::unmount(&dev, 0).unwrap();
    crate::mount::unmount(&proc, 0).unwrap();
}

#[test]
fn stacked_pivot_retains_old_root_for_propagation_and_detach() {
    struct Restore(crate::fs_context::State, Arc<crate::mount::shared::Store>);
    impl Drop for Restore {
        fn drop(&mut self) {
            crate::mount::shared::enter(self.1.id()).unwrap();
            crate::fs_context::update(|s| *s = self.0.clone());
        }
    }
    let _restore = Restore(
        crate::fs_context::read(Clone::clone),
        crate::mount::shared::get().unwrap(),
    );
    // pivot_root publishes into shared topology, not just fs_context. Keep this
    // test's pivot marker and detached old root out of subsequent test threads.
    crate::mount::unshare_namespace().unwrap();
    let f = Fixture::new();
    let old = fs::open("/", fs::O_PATH | fs::O_DIRECTORY, 0).unwrap();
    let new = fs::open(&f.target, fs::O_PATH | fs::O_DIRECTORY, 0).unwrap();
    fs::fchdir(new).unwrap();
    crate::mount::pivot_root(".", ".").unwrap();
    fs::fchdir(old).unwrap();
    assert!(fs::getcwd().starts_with("(unreachable)"));
    crate::mount::set_propagation(".", crate::mount::MS_REC | crate::mount::MS_SLAVE).unwrap();
    crate::mount::unmount(".", crate::mount::MNT_DETACH).unwrap();
    fs::fchdir(new).unwrap();
    assert_eq!(fs::getcwd(), "/");

    // docker exec enters this mount namespace in a fresh loader process. Its
    // fs_context has no copy of the original pivot caller's overlay root, so
    // path resolution must recover `/` from the shared mount topology.
    crate::fs_context::update(|state| {
        state.cwd = None;
        #[cfg(windows)]
        {
            state.cwd_object = None;
        }
        state.root = None;
        state.confined = false;
        state.overlay = None;
    });
    assert_eq!(
        crate::path::namespace_root_path().unwrap(),
        Some(f.target.clone())
    );
    let fd = fs::open("/data", fs::O_RDONLY, 0).unwrap();
    let mut bytes = [0; 32];
    let len = crate::read(fd, &mut bytes).unwrap();
    assert_eq!(&bytes[..len], b"lower data");
    for fd in [fd, new, old] {
        crate::close(fd).unwrap();
    }
}

#[test]
fn confined_openat2_merges_layers_and_copies_up_writes() {
    let f = Fixture::new();
    let root = fs::open(&f.target, fs::O_PATH | fs::O_DIRECTORY, 0).unwrap();
    for (name, expected) in [("/dir/a", b'a'), ("dir/b", b'b')] {
        let fd = fs::openat_resolved(root, name, fs::O_RDONLY, 0, 16 | 2).unwrap();
        let mut data = [0];
        assert_eq!(crate::read(fd, &mut data).unwrap(), 1);
        assert_eq!(data, [expected]);
        crate::close(fd).unwrap();
    }
    let fd = fs::openat_resolved(root, "data", fs::O_WRONLY | fs::O_TRUNC, 0, 8).unwrap();
    crate::write(fd, b"new").unwrap();
    crate::close(fd).unwrap();
    assert_eq!(f.read("data"), b"new");
    assert_eq!(
        std::fs::read(f.root.join("lower/data")).unwrap(),
        b"lower data"
    );
    crate::close(root).unwrap();
}

#[test]
fn confined_openat2_symlinks_and_root_boundaries() {
    let f = Fixture::new();
    for (name, target) in [
        ("absolute", "/data"),
        ("relative", "dir/../data"),
        ("loop", "loop"),
        ("escape", "../data"),
    ] {
        let mut link = prepare_create(&f.guest(name)).unwrap();
        crate::create_emulated_symlink(&link, target).unwrap();
        link.finish().unwrap();
    }
    let root = fs::open(&f.target, fs::O_PATH | fs::O_DIRECTORY, 0).unwrap();
    let inode = fs::stat(&f.guest("data")).unwrap().st_ino;
    for name in ["absolute", "relative", "escape", "../../data", "/data"] {
        let fd = fs::openat_resolved(root, name, fs::O_PATH, 0, 16 | 2).unwrap();
        assert_eq!(fs::fstat(fd).unwrap().st_ino, inode, "{name}");
        assert!(
            !crate::get(fd)
                .unwrap()
                .flags
                .contains(crate::FdFlags::CLOSE_ON_EXEC)
        );
        crate::close(fd).unwrap();
    }
    for name in ["absolute", "escape", "../data", "/data"] {
        assert_eq!(
            fs::openat_resolved(root, name, fs::O_PATH, 0, 8),
            Err(EXDEV),
            "{name}"
        );
    }
    for name in ["absolute", "loop"] {
        assert_eq!(
            fs::openat_resolved(root, name, fs::O_PATH, 0, 16 | 4),
            Err(crate::ELOOP)
        );
    }
    assert_eq!(
        fs::openat_resolved(root, "loop", fs::O_PATH, 0, 16),
        Err(crate::ELOOP)
    );
    let link = fs::openat_resolved(
        root,
        "absolute",
        fs::O_PATH | fs::O_NOFOLLOW | fs::O_CLOEXEC,
        0,
        16 | 4,
    )
    .unwrap();
    assert_eq!(fs::read_link_fd(link).unwrap(), "/data");
    assert!(
        crate::get(link)
            .unwrap()
            .flags
            .contains(crate::FdFlags::CLOSE_ON_EXEC)
    );
    crate::close(link).unwrap();
    for name in ["data/", "data/.", "data/.."] {
        assert_eq!(
            fs::openat_resolved(root, name, fs::O_PATH, 0, 16),
            Err(ENOTDIR)
        );
    }
    assert_eq!(
        fs::openat_resolved(root, "", fs::O_PATH, 0, 16),
        Err(ENOENT)
    );
    assert_eq!(
        fs::openat_resolved(root, "data", fs::O_PATH, 0, 32),
        Err(crate::EAGAIN)
    );
    crate::close(root).unwrap();
}

#[test]
fn confined_openat2_creates_files_and_anonymous_inodes() {
    let f = Fixture::new();
    let root = fs::open(&f.target, fs::O_PATH | fs::O_DIRECTORY, 0).unwrap();
    let flags = fs::O_CREAT | fs::O_EXCL | fs::O_RDWR;
    let fd = fs::openat_resolved(root, "dir/new", flags, 0o640, 16 | 2).unwrap();
    crate::write(fd, b"created").unwrap();
    assert_eq!(fs::fstat(fd).unwrap().st_mode & 0o7777, 0o640);
    crate::close(fd).unwrap();
    assert_eq!(f.read("dir/new"), b"created");
    assert!(!f.root.join("lower/dir/new").exists());
    assert_eq!(
        fs::openat_resolved(root, "dir/new", flags, 0o600, 16),
        Err(EEXIST)
    );
    let fd = fs::openat_resolved(
        root,
        "dir",
        0o20000000 | fs::O_DIRECTORY | fs::O_RDWR,
        0o600,
        16,
    )
    .unwrap();
    crate::write(fd, b"anonymous").unwrap();
    assert_eq!(fs::fstat(fd).unwrap().st_nlink, 0);
    crate::close(fd).unwrap();
    assert_eq!(std::fs::read_dir(f.root.join("work")).unwrap().count(), 0);
    let mut link = prepare_create(&f.guest("new-link")).unwrap();
    crate::create_emulated_symlink(&link, "dir/through-link").unwrap();
    link.finish().unwrap();
    drop(link);
    let fd = fs::openat_resolved(root, "new-link", fs::O_CREAT | fs::O_WRONLY, 0o600, 16).unwrap();
    crate::write(fd, b"target").unwrap();
    crate::close(fd).unwrap();
    assert_eq!(f.read("dir/through-link"), b"target");
    assert_eq!(
        fs::openat_resolved(root, "new-link", flags, 0o600, 16),
        Err(EEXIST)
    );
    crate::close(root).unwrap();
}

#[test]
fn confined_openat2_crosses_proc_without_following_magic_links() {
    let f = Fixture::new();
    crate::mount::proc_mount(&f.guest("empty"), 0, "").unwrap();
    let root = fs::open(&f.target, fs::O_PATH | fs::O_DIRECTORY, 0).unwrap();
    let status = fs::openat_resolved(root, "empty/self/status", fs::O_RDONLY, 0, 16 | 2).unwrap();
    assert!(crate::read(status, &mut [0; 64]).unwrap() > 0);
    crate::close(status).unwrap();
    assert_eq!(
        fs::openat_resolved(root, "empty/self/status", fs::O_PATH, 0, 16 | 4),
        Err(crate::ELOOP)
    );
    let name = format!("empty/self/fd/{root}");
    assert_eq!(
        fs::openat_resolved(root, &name, fs::O_PATH, 0, 16 | 2),
        Err(crate::ELOOP)
    );
    let fd = fs::openat_resolved(root, &name, fs::O_PATH | fs::O_NOFOLLOW, 0, 16 | 2).unwrap();
    assert_eq!(fs::fstat(fd).unwrap().st_mode & S_IFMT, S_IFLNK);
    crate::close(fd).unwrap();
    crate::close(root).unwrap();
    crate::mount::unmount(&f.guest("empty"), 0).unwrap();
}

#[test]
fn confined_openat2_absolute_paths_ignore_unused_dirfd() {
    let f = Fixture::new();
    for root in [fs::AT_FDCWD, -1] {
        let fd = fs::openat_resolved(root, &f.guest("dir/a"), fs::O_RDONLY, 0, 2).unwrap();
        let mut bytes = [0];
        assert_eq!(crate::read(fd, &mut bytes).unwrap(), 1);
        assert_eq!(bytes, [b'a']);
        crate::close(fd).unwrap();
    }
    let native = fs::open(
        &crate::to_guest_path(&f.root),
        fs::O_PATH | fs::O_DIRECTORY,
        0,
    )
    .unwrap();
    let fd = fs::openat_resolved(native, "created", fs::O_CREAT | fs::O_WRONLY, 0o600, 8).unwrap();
    crate::write(fd, b"native").unwrap();
    crate::close(fd).unwrap();
    assert_eq!(std::fs::read(f.root.join("created")).unwrap(), b"native");
    crate::close(native).unwrap();
}

#[test]
fn confined_openat2_reopens_cgroup_control_files() {
    let root = fs::open("/sys/fs/cgroup", fs::O_PATH | fs::O_DIRECTORY, 0).unwrap();
    let fd = fs::openat_resolved(
        root,
        "cgroup.controllers",
        fs::O_RDONLY | fs::O_CLOEXEC,
        0,
        8 | 1,
    )
    .unwrap();
    let mut bytes = [0; 128];
    let len = crate::read(fd, &mut bytes).unwrap();
    assert!(len > 0);
    assert!(std::str::from_utf8(&bytes[..len]).unwrap().contains("cpu"));
    crate::close(fd).unwrap();
    crate::close(root).unwrap();
}

#[test]
fn native_bind_remount_accepts_runc_flags_through_procfd() {
    let f = Fixture::new();
    let source = crate::to_guest_path(&f.root.join("source"));
    std::fs::write(f.root.join("source"), b"configuration").unwrap();
    crate::mount::bind(&source, &f.guest("data"), 4096 | 16384).unwrap();
    let fd = fs::open(&f.guest("data"), fs::O_PATH, 0).unwrap();
    let procfd = format!("/proc/thread-self/fd/{fd}");
    // Exact flags from runc's failed read-only configuration-file mount.
    let flags = 4096 | 32 | 16384 | (1 << 24);
    crate::mount::remount_bind(&procfd, flags | 1).unwrap();
    assert_eq!(fs::open(&f.guest("data"), fs::O_WRONLY, 0), Err(EROFS));
    let original = fs::open(&source, fs::O_WRONLY, 0).unwrap();
    crate::close(original).unwrap();
    crate::mount::remount_bind(&procfd, flags).unwrap();
    let writer = fs::open(&f.guest("data"), fs::O_WRONLY, 0).unwrap();
    crate::close(writer).unwrap();
    crate::close(fd).unwrap();
    crate::mount::unmount(&f.guest("data"), 0).unwrap();
}

#[test]
fn native_bind_remount_rec_does_not_remount_child_attachments() {
    let f = Fixture::new();
    std::fs::create_dir(f.root.join("directory")).unwrap();
    std::fs::write(f.root.join("directory/a"), b"parent").unwrap();
    std::fs::write(f.root.join("child"), b"child").unwrap();
    let source = crate::to_guest_path(&f.root.join("directory"));
    let child = crate::to_guest_path(&f.root.join("child"));
    crate::mount::bind(&source, &f.guest("dir"), 4096).unwrap();
    crate::mount::bind(&child, &f.guest("dir/a"), 4096).unwrap();
    crate::mount::remount_bind(&f.guest("dir"), 4096 | 32 | 16384 | 1).unwrap();
    assert_eq!(
        fs::open(&f.guest("dir/new"), fs::O_CREAT | fs::O_WRONLY, 0o600),
        Err(EROFS)
    );
    let writer = fs::open(&f.guest("dir/a"), fs::O_WRONLY, 0).unwrap();
    crate::write(writer, b"still writable").unwrap();
    crate::close(writer).unwrap();
    crate::mount::unmount(&f.guest("dir/a"), 0).unwrap();
    crate::mount::unmount(&f.guest("dir"), 0).unwrap();
}

#[test]
fn native_bind_mount_setattr_uses_procfd_attachment_and_enforces_readonly() {
    let f = Fixture::new();
    let source = crate::to_guest_path(&f.root.join("source"));
    std::fs::write(f.root.join("source"), b"configuration").unwrap();
    crate::mount::bind(&source, &f.guest("data"), 4096 | 16384).unwrap();
    let fd = fs::open(&f.guest("data"), fs::O_PATH, 0).unwrap();
    let procfd = format!("/proc/thread-self/fd/{fd}");
    let writer = fs::open(&f.guest("data"), fs::O_WRONLY, 0).unwrap();
    assert_eq!(
        crate::mount::api::set_attributes(&procfd, 0x8000, 1, 0, 0, 0),
        Err(crate::EBUSY)
    );
    crate::close(writer).unwrap();
    crate::mount::api::set_attributes(&procfd, 0x8000, 1, 0, 0, 0).unwrap();
    assert_eq!(fs::open(&f.guest("data"), fs::O_WRONLY, 0), Err(EROFS));
    // Read-only belongs to this attachment, not the underlying Windows file.
    let source_fd = fs::open(&source, fs::O_WRONLY, 0).unwrap();
    crate::write(source_fd, b"new").unwrap();
    crate::close(source_fd).unwrap();
    assert!(f.read("data").starts_with(b"new"));
    crate::mount::api::set_attributes(&procfd, 0x8000, 0, 1, 0, 0).unwrap();
    let writer = fs::open(&f.guest("data"), fs::O_WRONLY, 0).unwrap();
    crate::close(writer).unwrap();
    crate::close(fd).unwrap();
    // Path-based updates must accept the same native attachment too.
    crate::mount::api::set_attributes(&f.guest("data"), 0, 1, 0, 0, 0).unwrap();
    assert_eq!(fs::open(&f.guest("data"), fs::O_WRONLY, 0), Err(EROFS));
    crate::mount::unmount(&f.guest("data"), 0).unwrap();
    assert_eq!(f.read("data"), b"lower data");
}

#[test]
fn native_mount_setattr_through_fd_keeps_a_covered_attachment_identity() {
    let f = Fixture::new();
    let first = crate::to_guest_path(&f.root.join("first"));
    let second = crate::to_guest_path(&f.root.join("second"));
    std::fs::write(f.root.join("first"), b"first").unwrap();
    std::fs::write(f.root.join("second"), b"second").unwrap();
    crate::mount::bind(&first, &f.guest("data"), 4096).unwrap();
    let old = fs::open(&f.guest("data"), fs::O_PATH, 0).unwrap();
    crate::mount::bind(&second, &f.guest("data"), 4096).unwrap();
    crate::mount::api::set_attributes(&format!("/proc/self/fd/{old}"), 0, 1, 0, 0, 0).unwrap();
    let writer = fs::open(&f.guest("data"), fs::O_WRONLY, 0).unwrap();
    crate::close(writer).unwrap();
    assert_eq!(f.read("data"), b"second");
    crate::mount::unmount(&f.guest("data"), 0).unwrap();
    assert_eq!(f.read("data"), b"first");
    assert_eq!(fs::open(&f.guest("data"), fs::O_WRONLY, 0), Err(EROFS));
    crate::close(old).unwrap();
    crate::mount::unmount(&f.guest("data"), 0).unwrap();
}

#[test]
fn recursive_mount_setattr_rolls_back_overlay_and_native_bind_on_busy_writer() {
    let f = Fixture::new();
    let source = crate::to_guest_path(&f.root.join("source"));
    std::fs::write(f.root.join("source"), b"configuration").unwrap();
    crate::mount::bind(&source, &f.guest("dir/a"), 4096).unwrap();
    let writer = fs::open(&f.guest("dir/a"), fs::O_WRONLY, 0).unwrap();
    assert_eq!(
        crate::mount::api::set_attributes(&f.target, 0x8000, 1, 0, 0, 0),
        Err(crate::EBUSY)
    );
    let parent_writer = fs::open(&f.guest("data"), fs::O_WRONLY, 0).unwrap();
    crate::close(parent_writer).unwrap();
    crate::close(writer).unwrap();
    crate::mount::api::set_attributes(&f.target, 0x8000, 1, 0, 0, 0).unwrap();
    for path in ["data", "dir/a"] {
        assert_eq!(fs::open(&f.guest(path), fs::O_WRONLY, 0), Err(EROFS));
    }
    crate::mount::api::set_attributes(&f.target, 0x8000, 0, 1, 0, 0).unwrap();
    let writer = fs::open(&f.guest("dir/a"), fs::O_WRONLY, 0).unwrap();
    crate::close(writer).unwrap();
    crate::mount::unmount(&f.guest("dir/a"), 0).unwrap();
}

#[test]
fn confined_openat2_supports_file_bind_targets_and_readonly_remount() {
    let f = Fixture::new();
    let source = crate::to_guest_path(&f.root.join("source"));
    std::fs::write(f.root.join("source"), b"configuration").unwrap();
    let target = fs::open(&f.guest("data"), fs::O_PATH, 0).unwrap();
    crate::mount::bind(
        &source,
        &format!("/proc/thread-self/fd/{target}"),
        4096 | 16384,
    )
    .unwrap();
    assert_eq!(f.read("data"), b"configuration");
    let root = fs::open(&f.target, fs::O_PATH | fs::O_DIRECTORY, 0).unwrap();
    assert_eq!(
        fs::openat_resolved(root, "data", fs::O_RDONLY, 0, 16 | 1),
        Err(EXDEV)
    );
    let fd = fs::openat_resolved(root, "data", fs::O_RDONLY, 0, 16).unwrap();
    assert_eq!(
        crate::procfs::local_fd_link_target(fd).unwrap(),
        crate::procfs::local_fd_link_target(target).unwrap()
    );
    for flags in [
        crate::mount::MS_SHARED,
        crate::mount::MS_SLAVE,
        crate::mount::MS_PRIVATE | crate::mount::MS_REC,
    ] {
        crate::mount::set_propagation(&format!("/proc/thread-self/fd/{fd}"), flags).unwrap();
    }
    let mut bytes = [0; 13];
    assert_eq!(crate::read(fd, &mut bytes).unwrap(), 13);
    assert_eq!(&bytes, b"configuration");
    crate::close(fd).unwrap();
    crate::mount::remount_bind(&f.guest("data"), 4096 | 32 | 1).unwrap();
    assert_eq!(fs::open(&f.guest("data"), fs::O_WRONLY, 0), Err(EROFS));
    crate::mount::unmount(&f.guest("data"), 0).unwrap();
    assert_eq!(f.read("data"), b"lower data");
    let tree = crate::mount::api::open_tree(&source, 1 | 0x80000 | 0x8000).unwrap();
    assert_eq!(fs::fstat(tree).unwrap().st_mode & S_IFMT, fs::S_IFREG);
    let cloned = fs::open(&format!("/proc/self/fd/{tree}/."), fs::O_PATH, 0).unwrap();
    assert_eq!(fs::fstat(cloned).unwrap().st_mode & S_IFMT, fs::S_IFREG);
    crate::close(cloned).unwrap();
    crate::mount::api::move_tree(tree, &f.guest("data"), 0).unwrap();
    assert_eq!(f.read("data"), b"configuration");
    crate::mount::unmount(&f.guest("data"), 0).unwrap();
    crate::close(tree).unwrap();
    let file_tree = crate::mount::api::open_tree(&source, 1).unwrap();
    assert_eq!(
        crate::mount::api::move_tree(file_tree, &f.guest("dir"), 0),
        Err(ENOTDIR)
    );
    crate::close(file_tree).unwrap();
    for fd in [target, root] {
        crate::close(fd).unwrap();
    }
}

#[test]
fn confined_openat2_keeps_renamed_directory_and_mount_ancestors() {
    let f = Fixture::with_flags(super::super::features::REDIRECT | super::super::features::FOLLOW);
    let root = fs::open(&f.target, fs::O_PATH | fs::O_DIRECTORY, 0).unwrap();
    let dir = fs::open(&f.guest("dir"), fs::O_PATH | fs::O_DIRECTORY, 0).unwrap();
    fs::rename(&f.guest("dir"), &f.guest("moved")).unwrap();
    let fd = fs::openat_resolved(dir, "a", fs::O_RDONLY, 0, 8).unwrap();
    assert_eq!(crate::read(fd, &mut [0; 1]).unwrap(), 1);
    crate::close(fd).unwrap();
    crate::mount::tmpfs_mount(&f.guest("empty"), 0, "size=1m").unwrap();
    assert_eq!(
        fs::lstat(&f.guest("/empty")).unwrap().st_mode & S_IFMT,
        S_IFDIR
    );
    assert_eq!(
        fs::stat(&f.guest("empty")).unwrap().st_mode & S_IFMT,
        S_IFDIR
    );
    assert_eq!(
        fs::openat_resolved(root, "empty", fs::O_PATH, 0, 16 | 1),
        Err(EXDEV)
    );
    let fd = fs::openat_resolved(root, "empty/../data", fs::O_PATH, 0, 16).unwrap();
    assert_eq!(
        fs::fstat(fd).unwrap().st_ino,
        fs::stat(&f.guest("data")).unwrap().st_ino
    );
    crate::close(fd).unwrap();
    crate::mount::unmount(&f.guest("empty"), 0).unwrap();
    crate::mount::bind(&f.guest("moved"), &f.guest("empty"), 4096).unwrap();
    assert_eq!(
        fs::openat_resolved(root, "empty/a", fs::O_PATH, 0, 16 | 1),
        Err(EXDEV)
    );
    let fd = fs::openat_resolved(root, "empty/a", fs::O_RDONLY, 0, 16).unwrap();
    crate::close(fd).unwrap();
    crate::mount::unmount(&f.guest("empty"), 0).unwrap();
    for fd in [dir, root] {
        crate::close(fd).unwrap();
    }
}

#[test]
fn confined_openat2_reopens_masked_sysfs_directory() {
    let f = Fixture::new();
    let sys = f.guest("empty");
    let mask = f.guest("empty/firmware");
    crate::mount::sysfs_mount(&sys, 0, "").unwrap();
    let root = fs::open(&f.target, fs::O_PATH | fs::O_DIRECTORY, 0).unwrap();
    let before = fs::openat_resolved(root, "empty/firmware", fs::O_PATH, 0, 16 | 2).unwrap();
    crate::mount::tmpfs_mount(&mask, 1, "nr_blocks=1,nr_inodes=1").unwrap();
    let expected = fs::stat(&mask).unwrap();
    let reopened = fs::openat_resolved(root, "empty/firmware", fs::O_PATH, 0, 16 | 2).unwrap();
    let actual = fs::fstat(reopened).unwrap();
    assert_eq!(
        (actual.st_dev, actual.st_ino),
        (expected.st_dev, expected.st_ino)
    );
    assert_ne!(actual.st_dev, fs::fstat(before).unwrap().st_dev);
    assert_eq!(
        fs::openat_resolved(root, "empty/firmware", fs::O_PATH, 0, 16 | 1),
        Err(EXDEV)
    );
    let saved = crate::fs_context::read(Clone::clone);
    let confined = (|| {
        fs::chroot(&f.target)?;
        let jailed = fs::open("/", fs::O_PATH | fs::O_DIRECTORY, 0)?;
        let result = fs::openat_resolved(jailed, "/empty/firmware", fs::O_PATH, 0, 16 | 2);
        crate::close(jailed)?;
        result
    })();
    crate::fs_context::update(|s| *s = saved);
    crate::close(confined.unwrap()).unwrap();
    for fd in [reopened, before, root] {
        crate::close(fd).unwrap();
    }
    crate::mount::unmount(&mask, 0).unwrap();
    crate::mount::unmount(&sys, 0).unwrap();
}

#[test]
fn confined_openat2_reopens_mask_under_native_chroot() {
    let f = Fixture::new();
    let native = crate::path::default_system_root().join(f.root.file_name().unwrap());
    std::fs::create_dir_all(native.join("sys")).unwrap();
    let guest = crate::to_guest_path(&native);
    crate::mount::bind(&guest, &guest, 4096).unwrap();
    crate::mount::sysfs_mount(&format!("{guest}/sys"), 0, "").unwrap();
    let saved = crate::fs_context::read(Clone::clone);
    let result = (|| {
        fs::chroot(&guest)?;
        let root = fs::open("/", fs::O_PATH | fs::O_DIRECTORY, 0)?;
        crate::mount::tmpfs_mount("/sys/firmware", 1, "nr_blocks=1,nr_inodes=1")?;
        assert_eq!(
            fs::openat_resolved(root, "/sys/firmware", fs::O_PATH, 0, 16 | 1),
            Err(EXDEV)
        );
        let result = fs::openat_resolved(root, "/sys/firmware", fs::O_PATH, 0, 16 | 2);
        crate::close(root)?;
        result
    })();
    crate::fs_context::update(|s| *s = saved);
    crate::close(result.unwrap()).unwrap();
    crate::mount::unmount(&format!("{guest}/sys/firmware"), 0).unwrap();
    crate::mount::unmount(&format!("{guest}/sys"), 0).unwrap();
    crate::mount::unmount(&guest, 0).unwrap();
    std::fs::remove_dir_all(native).unwrap();
}

struct Fixture {
    root: PathBuf,
    target: String,
}

#[test]
#[ignore = "explicit native OverlayFS stat benchmark; run with --nocapture --test-threads=1"]
fn deep_overlay_stat_benchmark() {
    let f = Fixture::new();
    for depth in [4, 16] {
        let relative = (0..depth)
            .map(|index| format!("level{index}"))
            .collect::<Vec<_>>()
            .join("/");
        std::fs::create_dir_all(f.root.join("lower").join(&relative)).unwrap();
        std::fs::write(
            f.root.join("lower").join(&relative).join("file"),
            b"content",
        )
        .unwrap();
        let path = f.guest(&format!("{relative}/file"));
        let expected = fs::stat(&path).unwrap();
        for round in 0..4 {
            let start = std::time::Instant::now();
            for _ in 0..32 {
                let actual = fs::stat(&path).unwrap();
                assert_eq!(
                    (actual.st_dev, actual.st_ino, actual.st_size),
                    (expected.st_dev, expected.st_ino, 7)
                );
            }
            eprintln!(
                "overlay_stat depth={depth} round={round} count=32 elapsed_us={}",
                start.elapsed().as_micros()
            );
        }
    }
}

#[test]
fn component_walk_pins_do_not_cache_paths_across_operations() {
    let f = Fixture::new();
    let directory = f.root.join("upper/deep/child");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("file"), b"first").unwrap();
    let path = f.guest("deep/child/file");
    let before = fs::stat(&path).unwrap();
    // Replace through native inode operations: a persistent dentry cache that
    // depends only on this process's VFS notifications would miss this change.
    std::fs::rename(f.root.join("upper/deep"), f.root.join("upper/old")).unwrap();
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("file"), b"replacement").unwrap();
    let after = fs::stat(&path).unwrap();
    assert_ne!(before.st_ino, after.st_ino);
    assert_eq!(after.st_size, 11);
    assert_eq!(f.read("old/child/file"), b"first");
    assert_eq!(f.read("deep/child/../child/file"), b"replacement");
    let mut link = prepare_create(&f.guest("entry")).unwrap();
    crate::create_emulated_symlink(&link, "deep/child").unwrap();
    link.finish().unwrap();
    assert_eq!(f.read("entry/file"), b"replacement");
}
#[test]
#[ignore = "explicit native-prefix benchmark; run with --nocapture --test-threads=1"]
fn native_prefix_walk_benchmark() {
    let f = Fixture::new();
    for depth in [4, 16] {
        let relative = (0..depth)
            .map(|index| format!("level{index}"))
            .collect::<Vec<_>>()
            .join("/");
        let directory = f.root.join("native").join(relative);
        std::fs::create_dir_all(&directory).unwrap();
        let native = directory.join("file");
        std::fs::write(&native, b"content").unwrap();
        let path = crate::to_guest_path(&native);
        for round in 0..4 {
            let start = std::time::Instant::now();
            for _ in 0..32 {
                assert_eq!(resolve(&path, true, false).unwrap().unwrap().path, native);
            }
            eprintln!(
                "native_prefix depth={depth} round={round} count=32 elapsed_us={}",
                start.elapsed().as_micros()
            );
        }
    }
}

#[test]
fn native_prefix_walk_observes_links_bind_boundaries_and_replacement() {
    let f = Fixture::new();
    let base = f.root.join("native");
    std::fs::create_dir_all(base.join("deep/child")).unwrap();
    std::fs::write(base.join("deep/child/file"), b"first").unwrap();
    let guest = crate::to_guest_path(&base);
    crate::create_emulated_symlink(&base.join("entry"), "deep/child").unwrap();
    crate::create_emulated_symlink(&base.join("overlay"), &f.guest("dir")).unwrap();
    let read = |path: &str| {
        let fd = fs::open(path, fs::O_RDONLY, 0).unwrap();
        let mut bytes = [0; 32];
        let count = crate::read(fd, &mut bytes).unwrap();
        crate::close(fd).unwrap();
        bytes[..count].to_vec()
    };
    assert_eq!(read(&format!("{guest}/entry/file")), b"first");
    assert_eq!(read(&format!("{guest}/overlay/a")), b"a");
    assert_eq!(read(&format!("{guest}/deep/child/../child/file")), b"first");
    crate::mount::bind(&format!("{guest}/deep/child"), &f.guest("dir"), 4096).unwrap();
    assert_eq!(read(&f.guest("dir/file")), b"first");
    crate::mount::unmount(&f.guest("dir"), 0).unwrap();
    assert_eq!(read(&format!("{guest}/overlay/a")), b"a");
    std::fs::rename(base.join("deep"), base.join("old")).unwrap();
    std::fs::create_dir_all(base.join("deep/child")).unwrap();
    std::fs::write(base.join("deep/child/file"), b"replacement").unwrap();
    assert_eq!(read(&format!("{guest}/entry/file")), b"replacement");
    assert_eq!(read(&format!("{guest}/old/child/file")), b"first");
}

#[test]
fn native_metadata_operations_reuse_completed_lookup() {
    let f = Fixture::new();
    let directory = f.root.join("native/level1/level2/level3/level4");
    std::fs::create_dir_all(&directory).unwrap();
    let native = directory.join("file");
    std::fs::write(&native, b"content").unwrap();
    let path = crate::to_guest_path(&native);
    let before = RESOLUTION_WALKS.get();
    let started = std::time::Instant::now();
    for _ in 0..32 {
        assert_eq!(fs::stat(&path).unwrap().st_size, 7);
    }
    let stats = RESOLUTION_WALKS.get() - before;
    let stat_time = started.elapsed();
    let before = RESOLUTION_WALKS.get();
    let started = std::time::Instant::now();
    for _ in 0..32 {
        let description = crate::mount::native::prepare_open(&path, fs::O_RDONLY)
            .unwrap()
            .unwrap();
        assert_eq!(description.path, path);
        assert!(description.writer.is_none());
    }
    let opens = RESOLUTION_WALKS.get() - before;
    eprintln!(
        "32 stat: {stats} walks, {stat_time:?}; 32 open policies: {opens} walks, {:?}",
        started.elapsed()
    );
    assert_eq!(
        (stats, opens),
        (32, 32),
        "completed path walks must be reused within the operation"
    );
    let inode = fs::stat(&path).unwrap().st_ino;
    std::fs::rename(&native, directory.join("old-file")).unwrap();
    std::fs::write(&native, b"replacement").unwrap();
    let replaced = fs::stat(&path).unwrap();
    assert_ne!(replaced.st_ino, inode);
    assert_eq!(replaced.st_size, 11);
    let link = directory.join("link");
    crate::create_emulated_symlink(&link, "file").unwrap();
    let link = crate::to_guest_path(&link);
    assert_eq!(fs::stat(&link).unwrap().st_ino, replaced.st_ino);
    assert_eq!(fs::lstat(&link).unwrap().st_mode & S_IFMT, S_IFLNK);
}

impl Fixture {
    fn new() -> Self {
        Self::with_flags(0)
    }
    fn with_flags(flags: u64) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kinakaze-overlay-mounted-{}-{nonce}",
            std::process::id()
        ));
        for name in [
            "lower",
            "bottom",
            "upper",
            "work",
            "merged",
            "lower/dir",
            "upper/dir",
            "lower/empty",
        ] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        std::fs::write(root.join("lower/data"), b"lower data").unwrap();
        std::fs::write(root.join("bottom/bottom"), b"bottom").unwrap();
        std::fs::write(root.join("lower/dir/a"), b"a").unwrap();
        std::fs::write(root.join("upper/dir/b"), b"b").unwrap();
        let target = crate::to_guest_path(&root.join("merged"));
        crate::mount::overlay(
            &target,
            &[
                crate::to_guest_path(&root.join("lower")),
                crate::to_guest_path(&root.join("bottom")),
            ],
            Some(&crate::to_guest_path(&root.join("upper"))),
            Some(&crate::to_guest_path(&root.join("work"))),
            flags,
        )
        .unwrap();
        Self { root, target }
    }
    fn guest(&self, tail: &str) -> String {
        format!("{}/{tail}", self.target)
    }
    fn read(&self, tail: &str) -> Vec<u8> {
        let fd = fs::open(&self.guest(tail), fs::O_RDONLY, 0).unwrap();
        let mut data = vec![0; 100];
        let count = crate::read(fd, &mut data).unwrap();
        data.truncate(count);
        crate::close(fd).unwrap();
        data
    }
}

#[test]
fn userxattr_whiteouts_are_removed_with_their_empty_upper_directory() {
    let f = Fixture::with_flags(USER_XATTR);
    // A regular inode with user.overlay.whiteout is another valid whiteout
    // representation, not only our char-device representation.
    std::fs::remove_file(f.root.join("upper/dir/b")).unwrap();
    std::fs::write(f.root.join("upper/dir/a"), b"").unwrap();
    Attributes::open_host(&f.root.join("upper/dir/a"), true)
        .unwrap()
        .set(b"user.overlay.whiteout", b"", 0)
        .unwrap();
    assert_eq!(fs::read_directory(&f.guest("dir")).unwrap().len(), 2);
    fs::rmdir(&f.guest("dir")).unwrap();
    assert_eq!(fs::stat(&f.guest("dir")).unwrap_err(), ENOENT);
    assert_eq!(std::fs::read_dir(f.root.join("work")).unwrap().count(), 0);
    assert_eq!(std::fs::read(f.root.join("lower/dir/a")).unwrap(), b"a");
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = crate::mount::unmount(&self.target, 2);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn access_times_follow_live_mount_policy_without_touching_lower() {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::{FILE_WRITE_ATTRIBUTES, SetFileTime};
    let f = Fixture::with_flags(1 << 24);
    let old = FILETIME {
        dwLowDateTime: 123,
        dwHighDateTime: 30_000_000,
    };
    let stamp = |path: &Path| {
        let object = Object::open(path, ACCESS | FILE_WRITE_ATTRIBUTES).unwrap();
        assert_ne!(
            unsafe { SetFileTime(object.raw(), std::ptr::null(), &old, std::ptr::null()) },
            0
        );
        let stat = fs::stat_handle(object.raw(), false).unwrap();
        (stat.st_atime, stat.st_atime_nsec)
    };
    let atime = |path: &Path| {
        let st = fs::stat(&crate::to_guest_path(path)).unwrap();
        (st.st_atime, st.st_atime_nsec)
    };
    let lower = f.root.join("lower/data");
    let before = stamp(&lower);
    assert_eq!(f.read("data"), b"lower data");
    assert_eq!(atime(&lower), before);
    assert!(!f.root.join("upper/data").exists());
    let writer = fs::open(&f.guest("data"), fs::O_RDWR, 0).unwrap();
    crate::close(writer).unwrap();
    let upper = f.root.join("upper/data");
    let before = stamp(&upper);
    f.read("data");
    assert!(atime(&upper) > before);
    crate::mount::api::set_attributes(&f.target, 0, 0x10, 0x70, 0, 0).unwrap();
    let before = stamp(&upper);
    f.read("data");
    assert_eq!(atime(&upper), before);
    crate::mount::api::set_attributes(&f.target, 0, 0x20, 0x70, 0, 0).unwrap();
    let fd = fs::open(&f.guest("data"), fs::O_NOATIME, 0).unwrap();
    crate::read(fd, &mut [0; 8]).unwrap();
    crate::close(fd).unwrap();
    assert_eq!(atime(&upper), before);
    crate::mount::api::set_attributes(&f.target, 0, 0x80, 0, 0, 0).unwrap();
    let directory = f.root.join("upper/dir");
    let before = stamp(&directory);
    let fd = fs::open(&f.guest("dir"), fs::O_DIRECTORY, 0).unwrap();
    assert!(
        read_directory_bytes(fd, &mut [0; 1024], true)
            .unwrap()
            .unwrap()
            > 0
    );
    crate::close(fd).unwrap();
    assert_eq!(atime(&directory), before);
    assert!(
        crate::procfs::read_file("/proc/self/mountinfo")
            .unwrap()
            .windows(11)
            .any(|s| s == b"nodiratime ")
    );
}

#[test]
fn volatile_marker_blocks_reuse_and_volume_error_latches_across_all_descriptors() {
    let f = Fixture::with_flags(super::super::features::VOLATILE);
    assert!(f.root.join("work/work/incompat/volatile").is_dir());
    let fd = fs::open(&f.guest("data"), fs::O_RDWR, 0).unwrap();
    crate::write(fd, b"volatile").unwrap();
    assert_eq!(sync_descriptor(fd), Ok(Some(())));
    let object = Object::open(&f.root.join("upper/data"), ACCESS).unwrap();
    // Permission failures are not storage failures. A failure on the upper
    // filesystem poisons every subsequent sync, including a reopened fd.
    super::super::volatile::record(object.raw(), crate::EACCES);
    assert_eq!(sync_descriptor(fd), Ok(Some(())));
    super::super::volatile::record(object.raw(), crate::EIO);
    assert_eq!(sync_descriptor(fd), Err(crate::EIO));
    crate::close(fd).unwrap();
    let reader = fs::open(&f.guest("data"), 0, 0).unwrap();
    assert_eq!(sync_descriptor(reader), Err(crate::EIO));
    crate::close(reader).unwrap();
    let source = crate::mount::snapshot_list()
        .unwrap()
        .into_iter()
        .find(|m| m.target == f.target)
        .unwrap()
        .source;
    assert!(feature_options(&source).unwrap().contains("fsync=volatile"));
    crate::mount::unmount(&f.target, 2).unwrap();
    for flags in [0, super::super::features::VOLATILE] {
        assert_eq!(
            crate::mount::overlay(
                &f.target,
                &[crate::to_guest_path(&f.root.join("lower"))],
                Some(&crate::to_guest_path(&f.root.join("upper"))),
                Some(&crate::to_guest_path(&f.root.join("work"))),
                flags
            ),
            Err(crate::EINVAL)
        );
    }
}

#[test]
fn mounted_overlay_copy_up_whiteout_recreate_and_rename() {
    let f = Fixture::new();
    assert_eq!(f.read("bottom"), b"bottom");
    assert_eq!(f.read("data"), b"lower data");
    assert!(!f.root.join("upper/data").exists(), "read must not copy up");
    let old = fs::open(&f.guest("data"), fs::O_RDONLY, 0).unwrap();
    let before = fs::fstat(old).unwrap();
    let fd = fs::open(&f.guest("data"), fs::O_WRONLY | fs::O_TRUNC, 0).unwrap();
    crate::write(fd, b"upper").unwrap();
    assert_eq!(fs::fstat(fd).unwrap().st_ino, before.st_ino);
    crate::close(fd).unwrap();
    assert_eq!(
        std::fs::read(f.root.join("lower/data")).unwrap(),
        b"lower data"
    );
    let mut data = [0; 32];
    let n = crate::read(old, &mut data).unwrap();
    assert_eq!(&data[..n], b"lower data");
    assert_eq!(f.read("data"), b"upper");
    let mut names: Vec<_> = fs::read_directory(&f.guest("dir"))
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    names.sort();
    assert_eq!(names, [".", "..", "a", "b"]);
    fs::unlink(&f.guest("data")).unwrap();
    assert_eq!(fs::stat(&f.guest("data")).unwrap_err(), ENOENT);
    assert_eq!(
        fs::stat(&crate::to_guest_path(&f.root.join("upper/data")))
            .unwrap()
            .st_mode
            & S_IFMT,
        fs::S_IFCHR
    );
    let fd = fs::open(
        &f.guest("data"),
        fs::O_CREAT | fs::O_EXCL | fs::O_WRONLY,
        0o600,
    )
    .unwrap();
    crate::write(fd, b"new").unwrap();
    crate::close(fd).unwrap();
    assert_eq!(f.read("data"), b"new");
    fs::rename(&f.guest("data"), &f.guest("moved")).unwrap();
    assert_eq!(fs::stat(&f.guest("data")).unwrap_err(), ENOENT);
    assert_eq!(f.read("moved"), b"new");
    assert_eq!(fs::rename(&f.guest("dir"), &f.guest("dir2")), Err(EXDEV));
    fs::rmdir(&f.guest("empty")).unwrap();
    fs::mkdir(&f.guest("empty"), 0o750).unwrap();
    assert_eq!(fs::read_directory(&f.guest("empty")).unwrap().len(), 2);
    assert_eq!(std::fs::read_dir(f.root.join("work")).unwrap().count(), 0);
    assert_eq!(fs::fstat(old).unwrap().st_ino, before.st_ino);
    crate::close(old).unwrap();
}

#[test]
fn mounted_overlay_metadata_symlinks_links_and_layer_root_rename() {
    let f = Fixture::new();
    let path = f.guest("data");
    fs::set_mode(&path, 0o640).unwrap();
    assert_eq!(fs::stat(&path).unwrap().st_mode & 0o777, 0o640);
    assert_ne!(
        fs::stat(&crate::to_guest_path(&f.root.join("lower/data")))
            .unwrap()
            .st_mode
            & 0o777,
        0o640
    );
    assert!(hard_link(&path, &f.guest("linked")).unwrap());
    let fd = fs::open(&f.guest("linked"), fs::O_WRONLY, 0).unwrap();
    crate::write(fd, b"LINK").unwrap();
    crate::close(fd).unwrap();
    assert!(f.read("data").starts_with(b"LINK"));
    let mut link = prepare_create(&f.guest("symlink")).unwrap();
    crate::create_emulated_symlink(&link, "data").unwrap();
    link.finish().unwrap();
    drop(link);
    assert!(f.read("symlink").starts_with(b"LINK"));
    assert_eq!(
        fs::lstat(&f.guest("symlink")).unwrap().st_mode & S_IFMT,
        S_IFLNK
    );
    std::fs::rename(f.root.join("lower"), f.root.join("renamed-lower")).unwrap();
    assert_eq!(f.read("dir/a"), b"a");
    let source = crate::mount::overlay_location(&f.target)
        .unwrap()
        .unwrap()
        .0;
    instances().lock().unwrap().remove(&source);
    assert_eq!(
        f.read("dir/a"),
        b"a",
        "a fresh process must reopen renamed layer roots by ID"
    );
}

#[test]
fn metacopy_defers_data_and_redirect_rename_retains_lower_tree() {
    let f = Fixture::with_flags(
        super::super::features::METACOPY
            | super::super::features::REDIRECT
            | super::super::features::FOLLOW,
    );
    let original = fs::stat(&f.guest("data")).unwrap();
    fs::set_mode(&f.guest("data"), 0o640).unwrap();
    assert_eq!(
        std::fs::metadata(f.root.join("upper/data")).unwrap().len(),
        0
    );
    assert_eq!(
        fs::stat(&f.guest("data")).unwrap().st_size,
        original.st_size
    );
    assert_eq!(f.read("data"), b"lower data");
    fs::rename(&f.guest("data"), &f.guest("renamed")).unwrap();
    assert_eq!(f.read("renamed"), b"lower data");
    assert_eq!(
        std::fs::metadata(f.root.join("upper/renamed"))
            .unwrap()
            .len(),
        0
    );
    let fd = fs::open(&f.guest("renamed"), fs::O_RDWR, 0).unwrap();
    crate::write(fd, b"UPPER").unwrap();
    crate::close(fd).unwrap();
    assert_eq!(f.read("renamed"), b"UPPER data");
    assert_eq!(
        std::fs::read(f.root.join("lower/data")).unwrap(),
        b"lower data"
    );
    assert!(
        !Attributes::open_host(&f.root.join("upper/renamed"), false)
            .unwrap()
            .snapshot()
            .unwrap()
            .contains_key(b"trusted.overlay.metacopy".as_slice())
    );
    fs::rename(&f.guest("dir"), &f.guest("newdir")).unwrap();
    assert_eq!(f.read("newdir/a"), b"a");
    assert_eq!(f.read("newdir/b"), b"b");
    assert!(
        !f.root.join("upper/newdir/a").exists(),
        "directory rename must not eagerly copy its children"
    );
    assert_eq!(fs::stat(&f.guest("dir")).unwrap_err(), ENOENT);
    fs::rename(&f.guest("newdir"), &f.guest("again")).unwrap();
    assert_eq!(f.read("again/a"), b"a");
}

#[test]
fn rename_flags_preserve_destinations_and_exchange_open_inodes() {
    let f = Fixture::new();
    let old = fs::open(&f.guest("data"), fs::O_RDONLY, 0).unwrap();
    assert_eq!(
        fs::rename_with_flags(&f.guest("data"), &f.guest("bottom"), 1),
        Err(EEXIST)
    );
    fs::rename_with_flags(&f.guest("data"), &f.guest("bottom"), 2).unwrap();
    assert_eq!(f.read("data"), b"bottom");
    assert_eq!(f.read("bottom"), b"lower data");
    assert_eq!(
        descriptor_path(old).unwrap().as_deref(),
        Some(f.guest("bottom").as_str())
    );
    fs::rename_with_flags(&f.guest("data"), &f.guest("whiteout-move"), 4).unwrap();
    assert_eq!(fs::stat(&f.guest("data")).unwrap_err(), ENOENT);
    assert_eq!(std::fs::read_dir(f.root.join("work")).unwrap().count(), 0);
    crate::close(old).unwrap();
}

#[test]
fn metacopy_pins_refresh_after_alias_promotion_and_unlink() {
    use super::super::features::*;
    let f = Fixture::with_flags(INDEX | METACOPY | REDIRECT | FOLLOW);
    std::fs::hard_link(f.root.join("lower/data"), f.root.join("lower/alias")).unwrap();
    fs::set_mode(&f.guest("data"), 0o640).unwrap();
    let fd = fs::open(&f.guest("data"), fs::O_RDONLY, 0).unwrap();
    let old = reference(crate::get(fd).unwrap()).unwrap().unwrap();
    assert!(old.location.node.as_ref().unwrap().is_metacopy());
    let write = fs::open(&f.guest("alias"), fs::O_WRONLY | fs::O_TRUNC, 0).unwrap();
    crate::write(write, b"NEW").unwrap();
    crate::close(write).unwrap();
    assert_eq!(f.read("data"), b"NEW");
    fs::unlink(&f.guest("data")).unwrap();
    fs::unlink(&f.guest("alias")).unwrap();
    assert_eq!(
        old.location
            .metadata(old.location.node.as_ref().unwrap())
            .unwrap()
            .st_size,
        3
    );
    let (reopened, _) =
        reopen_object(&old, crate::get(fd).unwrap().raw as _, false, false).unwrap();
    assert_eq!(fs::stat_handle(reopened.raw(), false).unwrap().st_size, 3);
    crate::close(fd).unwrap();
}

#[test]
fn index_counts_aliases_deleted_before_the_first_write() {
    let f = Fixture::with_flags(super::super::features::INDEX);
    std::fs::hard_link(f.root.join("lower/data"), f.root.join("lower/alias")).unwrap();
    fs::unlink(&f.guest("data")).unwrap();
    assert_eq!(fs::stat(&f.guest("alias")).unwrap().st_nlink, 1);
    let fd = fs::open(&f.guest("alias"), fs::O_WRONLY, 0).unwrap();
    crate::write(fd, b"NEW").unwrap();
    fs::unlink(&f.guest("alias")).unwrap();
    assert_eq!(fs::fstat(fd).unwrap().st_nlink, 0);
    crate::close(fd).unwrap();
}

#[test]
fn index_preserves_lower_hardlink_identity_and_remount_origin() {
    let f = Fixture::with_flags(super::super::features::INDEX);
    std::fs::hard_link(f.root.join("lower/data"), f.root.join("lower/alias")).unwrap();
    let old = fs::open(&f.guest("data"), fs::O_RDONLY, 0).unwrap();
    let fd = fs::open(&f.guest("data"), fs::O_WRONLY, 0).unwrap();
    crate::write(fd, b"UPPER").unwrap();
    crate::close(fd).unwrap();
    assert_eq!(f.read("alias"), b"UPPER data");
    assert_eq!(fs::stat(&f.guest("alias")).unwrap().st_nlink, 2);
    let fd = fs::open(&f.guest("alias"), fs::O_RDWR, 0).unwrap();
    crate::close(fd).unwrap();
    assert_eq!(
        fs::stat(&f.guest("alias")).unwrap().st_ino,
        fs::stat(&f.guest("data")).unwrap().st_ino
    );
    fs::unlink(&f.guest("data")).unwrap();
    assert_eq!(fs::fstat(old).unwrap().st_nlink, 1);
    assert_eq!(fs::stat(&f.guest("alias")).unwrap().st_nlink, 1);
    crate::mount::unmount(&f.target, 0).unwrap();
    crate::mount::overlay(
        &f.target,
        &[
            crate::to_guest_path(&f.root.join("lower")),
            crate::to_guest_path(&f.root.join("bottom")),
        ],
        Some(&crate::to_guest_path(&f.root.join("upper"))),
        Some(&crate::to_guest_path(&f.root.join("work"))),
        super::super::features::INDEX,
    )
    .unwrap();
    assert_eq!(f.read("alias"), b"UPPER data");
    fs::unlink(&f.guest("alias")).unwrap();
    assert_eq!(fs::fstat(old).unwrap().st_nlink, 0);
    crate::close(old).unwrap();
    crate::mount::unmount(&f.target, 0).unwrap();
    assert_eq!(
        crate::mount::overlay(
            &f.target,
            &[crate::to_guest_path(&f.root.join("bottom"))],
            Some(&crate::to_guest_path(&f.root.join("upper"))),
            Some(&crate::to_guest_path(&f.root.join("work"))),
            super::super::features::INDEX
        ),
        Err(crate::ESTALE)
    );
}

#[test]
fn export_handles_follow_copy_up_rename_and_reject_deleted_inodes() {
    let flags = super::super::features::INDEX
        | super::super::features::NFS_EXPORT
        | super::super::features::UUID_ON
        | super::super::features::XINO;
    let f = Fixture::with_flags(flags);
    let mount = fs::open(&f.target, fs::O_RDONLY | fs::O_DIRECTORY, 0).unwrap();
    let fsid = descriptor_filesystem(mount).unwrap().unwrap().1;
    let (handle, _) = encode_handle(&f.guest("data"), true).unwrap();
    fs::rename(&f.guest("data"), &f.guest("renamed-export")).unwrap();
    let fd = open_handle(mount, &handle, fs::O_RDONLY).unwrap();
    let mut bytes = [0; 10];
    assert_eq!(crate::read(fd, &mut bytes).unwrap(), 10);
    assert_eq!(&bytes, b"lower data");
    crate::close(fd).unwrap();
    crate::mount::unmount(&f.target, 0).unwrap();
    crate::mount::overlay(
        &f.target,
        &[
            crate::to_guest_path(&f.root.join("lower")),
            crate::to_guest_path(&f.root.join("bottom")),
        ],
        Some(&crate::to_guest_path(&f.root.join("upper"))),
        Some(&crate::to_guest_path(&f.root.join("work"))),
        flags,
    )
    .unwrap();
    let fresh_mount = fs::open(&f.target, fs::O_RDONLY | fs::O_DIRECTORY, 0).unwrap();
    assert_eq!(descriptor_filesystem(fresh_mount).unwrap().unwrap().1, fsid);
    let fd = open_handle(fresh_mount, &handle, fs::O_RDONLY).unwrap();
    crate::close(fd).unwrap();
    fs::unlink(&f.guest("renamed-export")).unwrap();
    assert_eq!(
        open_handle(fresh_mount, &handle, fs::O_RDONLY),
        Err(crate::ESTALE)
    );
    let (directory, _) = encode_handle(&f.guest("dir"), true).unwrap();
    let fd = open_handle(fresh_mount, &directory, fs::O_RDONLY | fs::O_DIRECTORY).unwrap();
    crate::close(fd).unwrap();
    crate::close(mount).unwrap();
    crate::close(fresh_mount).unwrap();
}

#[test]
fn verity_metacopy_checks_digest_and_require_copies_plain_data() {
    use super::super::features::*;
    let f = Fixture::with_flags(METACOPY | REDIRECT | FOLLOW | VERITY | VERITY_REQUIRE);
    fs::set_mode(&f.guest("data"), 0o640).unwrap();
    assert_eq!(
        std::fs::metadata(f.root.join("upper/data")).unwrap().len(),
        10,
        "require uses full copy for a source without verity"
    );
    let lower_path = crate::to_guest_path(&f.root.join("bottom/bottom"));
    let lower = fs::open(&lower_path, fs::O_RDONLY, 0).unwrap();
    fs::verity::enable(lower, 1, 4096, b"").unwrap();
    crate::close(lower).unwrap();
    fs::set_mode(&f.guest("bottom"), 0o640).unwrap();
    assert_eq!(
        std::fs::metadata(f.root.join("upper/bottom"))
            .unwrap()
            .len(),
        0
    );
    let attrs = Attributes::open_host(&f.root.join("upper/bottom"), true).unwrap();
    let mut marker = attrs.get(b"trusted.overlay.metacopy").unwrap();
    assert_eq!(marker.len(), 36);
    assert_eq!(f.read("bottom"), b"bottom");
    marker[4] ^= 1;
    attrs.set(b"trusted.overlay.metacopy", &marker, 0).unwrap();
    assert_eq!(fs::open(&f.guest("bottom"), fs::O_RDONLY, 0), Err(EIO));
}

#[test]
fn xino_maps_distinct_filesystems_and_overflow_keeps_backing_device() {
    use super::super::features::*;
    let f = Fixture::with_flags(XINO);
    let location = resolve(&f.guest("data"), true, false)
        .unwrap()
        .unwrap()
        .location
        .unwrap();
    let mut node = location.lookup().unwrap();
    let mut roots = node.context.as_ref().unwrap().roots.clone();
    roots[0].metadata.st_dev = 10;
    roots[1].metadata.st_dev = 20;
    roots[2].metadata.st_dev = 20;
    node.context = Some(Arc::new(super::super::LookupContext {
        roots,
        flags: XINO,
        writable: true,
        volumes: vec![[1; 16], [2; 16], [2; 16]],
        index: None,
    }));
    let a = node.map_inode(10, 123, 777);
    let b = node.map_inode(20, 123, 777);
    assert_eq!(a.0, 777);
    assert_eq!(b.0, 777);
    assert_ne!(a.1, b.1);
    assert_eq!(node.map_inode(20, u64::MAX, 777), (20, u64::MAX));
}

#[test]
fn data_only_layers_supply_bytes_without_exposing_names_or_metadata() {
    use super::super::features::*;
    for flags in [
        1 << 54,
        (1 << 54) | USER_XATTR,
        (1 << 54) | METACOPY | REDIRECT | FOLLOW,
    ] {
        let f = Fixture::with_flags(flags);
        let prefix = if flags & USER_XATTR != 0 {
            "user.overlay."
        } else {
            "trusted.overlay."
        };
        let attrs = Attributes::open_host(&f.root.join("lower/data"), true).unwrap();
        attrs
            .set(format!("{prefix}metacopy").as_bytes(), b"", 0)
            .unwrap();
        attrs
            .set(format!("{prefix}redirect").as_bytes(), b"/bottom", 0)
            .unwrap();
        let meta = fs::stat(&crate::to_guest_path(&f.root.join("lower/data"))).unwrap();
        assert_eq!(f.read("data"), b"bottom");
        let stat = fs::stat(&f.guest("data")).unwrap();
        assert_eq!(stat.st_ino, meta.st_ino);
        assert_eq!(stat.st_size, 6);
        assert_eq!(fs::stat(&f.guest("bottom")).unwrap_err(), ENOENT);
        assert!(
            !fs::read_directory(&f.target)
                .unwrap()
                .iter()
                .any(|entry| entry.name == "bottom")
        );
        let fd = fs::open(&f.guest("data"), fs::O_WRONLY, 0).unwrap();
        crate::write(fd, b"UPPER!").unwrap();
        crate::close(fd).unwrap();
        assert_eq!(f.read("data"), b"UPPER!");
        assert_eq!(
            std::fs::read(f.root.join("bottom/bottom")).unwrap(),
            b"bottom"
        );
    }
}

#[test]
fn data_only_redirects_reject_relative_paths_and_escape_components() {
    let f = Fixture::with_flags(1 << 54);
    let attrs = Attributes::open_host(&f.root.join("lower/data"), true).unwrap();
    attrs.set(b"trusted.overlay.metacopy", b"", 0).unwrap();
    for redirect in ["bottom", "/../bottom", "/bottom/../bottom", "/missing"] {
        attrs
            .set(b"trusted.overlay.redirect", redirect.as_bytes(), 0)
            .unwrap();
        assert_eq!(
            fs::open(&f.guest("data"), fs::O_RDONLY, 0),
            Err(EIO),
            "{redirect}"
        );
    }
}

#[test]
fn index_installation_rolls_back_an_unpublished_directory() {
    let f = Fixture::with_flags(super::super::features::INDEX);
    let location = resolve(&f.guest("dir"), true, false)
        .unwrap()
        .unwrap()
        .location
        .unwrap();
    let mut lower = location.instance.root.clone();
    lower.entries.remove(0);
    let source = lower.child("dir").unwrap();
    let stage = f.root.join("work/test-stage");
    std::fs::create_dir(&stage).unwrap();
    // The rollback guard removes only the index inode it created.
    let index = location
        .instance
        .root
        .context
        .as_ref()
        .unwrap()
        .index
        .as_ref()
        .unwrap();
    let before = index.directory.entries().unwrap().len();
    let object = Object::open(&stage, ACCESS).unwrap();
    let pending = index
        .install(&source.origin().unwrap(), &object, 1)
        .unwrap();
    assert_eq!(index.directory.entries().unwrap().len(), before + 1);
    drop(pending);
    assert_eq!(index.directory.entries().unwrap().len(), before);
}

#[test]
fn strict_copy_up_flushes_metadata_directories_and_index() {
    use super::super::features::*;
    let f = Fixture::with_flags(FSYNC_STRICT | INDEX | METACOPY | REDIRECT | FOLLOW);
    let fd = fs::open(&f.guest("data"), fs::O_RDONLY, 0).unwrap();
    fs::set_mode(&f.guest("data"), 0o600).unwrap();
    assert_eq!(
        std::fs::metadata(f.root.join("upper/data")).unwrap().len(),
        0
    );
    assert_eq!(sync_descriptor(fd), Ok(Some(())));
    assert_eq!(
        std::fs::metadata(f.root.join("upper/data")).unwrap().len(),
        0
    );
    fs::set_mode(&f.guest("empty"), 0o700).unwrap();
    let dir = fs::open(&f.guest("empty"), fs::O_DIRECTORY, 0).unwrap();
    assert_eq!(sync_descriptor(dir), Ok(Some(())));
    let write = fs::open(&f.guest("data"), fs::O_WRONLY, 0).unwrap();
    crate::write(write, b"NEW").unwrap();
    assert_eq!(sync_descriptor(write), Ok(Some(())));
    for fd in [fd, dir, write] {
        crate::close(fd).unwrap();
    }
}

#[test]
fn export_empty_path_pins_the_inode_and_symlinks_do_not_follow() {
    use super::super::features::*;
    let f = Fixture::with_flags(INDEX | NFS_EXPORT);
    let root = fs::open(&f.target, fs::O_DIRECTORY, 0).unwrap();
    let old = fs::open(&f.guest("data"), fs::O_RDONLY, 0).unwrap();
    let (handle, _) = encode_handle_fd(old).unwrap();
    fs::unlink(&f.guest("data")).unwrap();
    let new = fs::open(
        &f.guest("data"),
        fs::O_CREAT | fs::O_EXCL | fs::O_WRONLY,
        0o600,
    )
    .unwrap();
    crate::close(new).unwrap();
    assert_eq!(encode_handle_fd(old).unwrap().0, handle);
    assert_eq!(open_handle(root, &handle, fs::O_RDONLY), Err(crate::ESTALE));
    let mut link = prepare_create(&f.guest("symlink")).unwrap();
    crate::create_emulated_symlink(&link, "data").unwrap();
    link.finish().unwrap();
    drop(link);
    let (handle, _) = encode_handle(&f.guest("symlink"), false).unwrap();
    assert_eq!(open_handle(root, &handle, fs::O_RDONLY), Err(crate::ELOOP));
    let link = open_handle(root, &handle, fs::O_PATH).unwrap();
    assert_eq!(fs::fstat(link).unwrap().st_mode & S_IFMT, S_IFLNK);
    for fd in [root, old, link] {
        crate::close(fd).unwrap();
    }
}

#[test]
fn xino_and_export_work_across_real_native_volumes() {
    use super::super::features::*;
    let f = Fixture::new();
    let other_root = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("diagnostics")
        .join(f.root.file_name().unwrap());
    std::fs::create_dir_all(&other_root).unwrap();
    struct OwnedDirectory(PathBuf);
    impl Drop for OwnedDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _other = OwnedDirectory(other_root.clone());
    let here = Object::open(&f.root, ACCESS).unwrap();
    let there = Object::open(&other_root, ACCESS).unwrap();
    if fs::stat_handle(here.raw(), false).unwrap().st_dev
        == fs::stat_handle(there.raw(), false).unwrap().st_dev
    {
        eprintln!("cross-volume overlay fixture unavailable: temp and build use the same volume");
        return;
    }
    std::fs::write(other_root.join("remote"), b"remote bytes").unwrap();
    crate::mount::unmount(&f.target, 0).unwrap();
    let flags = INDEX | XINO | NFS_EXPORT | UUID_OFF;
    crate::mount::overlay(
        &f.target,
        &[
            crate::to_guest_path(&other_root),
            crate::to_guest_path(&f.root.join("lower")),
        ],
        Some(&crate::to_guest_path(&f.root.join("upper"))),
        Some(&crate::to_guest_path(&f.root.join("work"))),
        flags,
    )
    .unwrap();
    let before = fs::stat(&f.guest("remote")).unwrap();
    assert_eq!(before.st_dev, fs::stat(&f.guest("data")).unwrap().st_dev);
    let (handle, _) = encode_handle(&f.guest("remote"), false).unwrap();
    assert_ne!(
        super::super::identity::Identity::decode(&handle)
            .unwrap()
            .uuid,
        [0; 16],
        "mixed lower volumes cannot ignore UUIDs"
    );
    let fd = fs::open(&f.guest("remote"), fs::O_WRONLY, 0).unwrap();
    crate::write(fd, b"UPPER").unwrap();
    crate::close(fd).unwrap();
    let after = fs::stat(&f.guest("remote")).unwrap();
    assert_eq!((before.st_dev, before.st_ino), (after.st_dev, after.st_ino));
    let root_fd = fs::open(&f.target, fs::O_DIRECTORY, 0).unwrap();
    let fd = open_handle(root_fd, &handle, fs::O_RDONLY).unwrap();
    let mut bytes = [0; 12];
    crate::read(fd, &mut bytes).unwrap();
    assert_eq!(&bytes, b"UPPERe bytes");
    crate::close(fd).unwrap();
    crate::close(root_fd).unwrap();
    assert_eq!(
        std::fs::read(other_root.join("remote")).unwrap(),
        b"remote bytes"
    );
}
