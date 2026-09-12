//! Pure attachment-tree tests; these do not publish into the live namespace.

use super::*;

#[test]
fn tmpfs_attachment_hint_rechecks_unmount_and_does_not_pin_a_namespace() {
    let original = shared::get().unwrap();
    let volume = shared::next_group().unwrap();
    unshare_namespace().unwrap();
    let first = shared::get().unwrap();
    let table = vec![MountPoint {
        meta: MountMetadata::default(),
        id: FIRST_DYNAMIC_MOUNT_ID,
        parent: ROOT_MOUNT_ID,
        source: format!("tmpfs:{volume}:1"),
        target: "/attachment-hint-test".into(),
        flags: MS_TMPFS,
    }];
    first.update(|_| Ok((encode_table(&table)?, ()))).unwrap();
    let mut hint = None;
    assert!(tmpfs_attached(volume, &mut hint).unwrap());
    assert_eq!(hint, Some(first.id()));
    let started = std::time::Instant::now();
    for _ in 0..100 {
        assert!(tmpfs_attached(volume, &mut hint).unwrap());
    }
    eprintln!(
        "100 verified tmpfs attachment hints: {:?}",
        started.elapsed()
    );

    unshare_namespace().unwrap();
    let second = shared::get().unwrap();
    first.update(|_| Ok((encode_table(&[])?, ()))).unwrap();
    assert!(tmpfs_attached(volume, &mut hint).unwrap());
    assert_eq!(
        hint,
        Some(second.id()),
        "an unmount must invalidate the positive hint"
    );

    // Only the numeric ID is remembered. Once the last real namespace owner
    // leaves, its section and mounts must disappear despite the cached hit.
    let second_id = second.id();
    shared::enter(original.id()).unwrap();
    drop(second);
    drop(first);
    assert!(matches!(shared::namespace(second_id), Err(crate::ENOENT)));
    assert!(!tmpfs_attached(volume, &mut hint).unwrap());
    assert_eq!(hint, None);
}

#[derive(Default)]
struct Tree {
    points: Vec<MountPoint>,
    next: u64,
}

impl Tree {
    fn new() -> Self {
        Self {
            next: FIRST_DYNAMIC_MOUNT_ID,
            ..Self::default()
        }
    }

    fn bind(&mut self, source: &str, target: &str, recursive: bool) -> u64 {
        let id = self.next;
        bind_in(
            &mut self.points,
            &mut self.next,
            source,
            target,
            MS_BIND | if recursive { MS_REC } else { 0 },
        )
        .unwrap();
        id
    }

    fn translate(&self, path: &str) -> String {
        translate_in(&self.points, path).unwrap()
    }

    fn unmount(&mut self, path: &str, flags: i32) -> Result<(), i32> {
        unmount_in(&mut self.points, path, flags)
    }

    fn restore(&mut self) {
        self.points =
            decode_table(&encode_table_with_next_id(&self.points, self.next).unwrap()).unwrap();
    }
}

#[test]
fn new_shared_bind_does_not_receive_its_own_mount_event() {
    let original = open_namespace(crate::FdFlags::NONE).unwrap();
    unshare_namespace().unwrap();
    let root = std::env::temp_dir().join(format!(
        "kinakaze-propagation-self-{}-{}",
        std::process::id(),
        shared::next_group().unwrap()
    ));
    for tail in ["backing/sub", "backing/tmp/mount", "view", "peer"] {
        std::fs::create_dir_all(root.join(tail)).unwrap();
    }
    let path = |tail: &str| crate::to_guest_path(&root.join(tail));
    bind(&path("backing"), &path("view"), MS_BIND).unwrap();
    set_propagation(&path("view"), MS_SHARED).unwrap();
    bind(&path("view"), &path("peer"), MS_BIND).unwrap();
    bind(&path("view/sub"), &path("view/tmp/mount"), MS_BIND).unwrap();
    let mounts = snapshot().unwrap();
    let own = visible_mount(&mounts, &path("view/tmp/mount"))
        .unwrap()
        .unwrap();
    let peer = visible_mount(&mounts, &path("peer/tmp/mount"))
        .unwrap()
        .unwrap();
    assert_ne!(
        own.id, peer.id,
        "the event must reach the pre-existing peer"
    );
    assert_eq!(
        mounts.iter().filter(|p| p.parent == own.id).count(),
        0,
        "a new shared bind must not acquire a recursively propagated child"
    );
    assert_eq!(mounts.iter().filter(|p| p.parent == peer.id).count(), 0);
    unmount(&path("view/tmp/mount"), 0).unwrap();
    assert_eq!(unmount(&path("peer/tmp/mount"), 0), Err(EINVAL));
    unmount(&path("peer"), 0).unwrap();
    unmount(&path("view"), 0).unwrap();
    enter_namespace(original).unwrap();
    crate::close(original).unwrap();
    for tail in [
        "backing/tmp/mount",
        "backing/tmp",
        "backing/sub",
        "backing",
        "view",
        "peer",
        "",
    ] {
        std::fs::remove_dir(root.join(tail)).unwrap();
    }
}

#[test]
fn shared_mount_events_reach_peers_and_slaves_but_do_not_flow_upstream() {
    crate::fs_context::unshare();
    let saved_fs = crate::fs_context::read(Clone::clone);
    crate::path::set_system_root(crate::path::default_system_root().to_path_buf());
    let original = open_namespace(crate::FdFlags::NONE).unwrap();
    unshare_namespace().unwrap();
    // setns installs the namespace root. Keep actual VFS lookups inside that
    // root rather than relying on standalone access to arbitrary host paths.
    let root = crate::path::default_system_root().join(format!(
        "kinakaze-propagation-{}-{}",
        std::process::id(),
        shared::next_group().unwrap()
    ));
    for tail in ["base/a", "base/b", "base/c", "target", "payload"] {
        std::fs::create_dir_all(root.join(tail)).unwrap();
    }
    std::fs::write(root.join("payload/value"), b"propagated").unwrap();
    let base = crate::to_guest_path(&root);
    let guest = |tail: &str| format!("{base}/{tail}");
    bind(&guest("base"), &guest("target"), MS_BIND).unwrap();
    set_propagation(&guest("target"), MS_SHARED).unwrap();
    set_propagation(&guest("target"), MS_SLAVE).unwrap();
    let table = snapshot().unwrap();
    let single = visible_mount(&table, &guest("target")).unwrap().unwrap();
    assert_eq!((single.meta.peer, single.meta.master), (0, 0));
    assert_ne!(single.flags & MS_PRIVATE, 0);
    set_propagation(&guest("target"), MS_SHARED).unwrap();
    let parent = open_namespace(crate::FdFlags::NONE).unwrap();
    unshare_namespace().unwrap();
    let child = open_namespace(crate::FdFlags::NONE).unwrap();
    // A cloned namespace remains in the same peer group.
    bind(&guest("payload"), &guest("target/a"), MS_BIND).unwrap();
    enter_namespace(parent).unwrap();
    assert_eq!(
        translate(&guest("target/a/value")).unwrap(),
        guest("payload/value")
    );
    assert_eq!(
        api::move_path(&guest("target/a"), &guest("target/b"), 0),
        Err(EINVAL)
    );
    unmount(&guest("target/a"), 0).unwrap();
    enter_namespace(child).unwrap();
    assert_eq!(
        translate(&guest("target/a/value")).unwrap(),
        guest("base/a/value")
    );
    set_propagation(&guest("target"), MS_SLAVE).unwrap();
    bind(&guest("payload"), &guest("target/b"), MS_BIND).unwrap();
    enter_namespace(parent).unwrap();
    assert_eq!(
        translate(&guest("target/b/value")).unwrap(),
        guest("base/b/value")
    );
    bind(&guest("payload"), &guest("target/a"), MS_BIND).unwrap();
    enter_namespace(child).unwrap();
    assert_eq!(
        translate(&guest("target/a/value")).unwrap(),
        guest("payload/value")
    );
    set_propagation(&guest("target"), MS_PRIVATE | MS_REC).unwrap();
    enter_namespace(parent).unwrap();
    bind(&guest("payload"), &guest("target/c"), MS_BIND).unwrap();
    enter_namespace(child).unwrap();
    assert_eq!(
        translate(&guest("target/c/value")).unwrap(),
        guest("base/c/value")
    );
    unmount(&guest("target"), MNT_DETACH).unwrap();
    enter_namespace(parent).unwrap();
    unmount(&guest("target"), MNT_DETACH).unwrap();
    enter_namespace(original).unwrap();
    for fd in [child, parent, original] {
        crate::close(fd).unwrap();
    }
    crate::fs_context::update(|state| *state = saved_fs);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn later_parent_covers_older_child_without_reparenting_it() {
    let mut tree = Tree::new();
    let child = tree.bind("/storage/child", "/mount/child", false);
    let cover = tree.bind("/storage/parent", "/mount", false);
    tree.restore();
    assert_eq!(
        tree.points
            .iter()
            .map(|point| (point.id, point.parent))
            .collect::<Vec<_>>(),
        [(child, ROOT_MOUNT_ID), (cover, ROOT_MOUNT_ID)]
    );
    assert_eq!(
        tree.translate("/mount/child/file"),
        "/storage/parent/child/file"
    );
    assert_eq!(tree.unmount("/mount/child", 0), Err(EINVAL));
    assert_eq!(tree.unmount("/mount", 0), Ok(()));
    assert_eq!(tree.translate("/mount/child/file"), "/storage/child/file");
}

#[test]
fn stacking_hides_old_children_and_pop_restores_them() {
    let mut tree = Tree::new();
    let base = tree.bind("/storage/base", "/mount", false);
    let child = tree.bind("/storage/child", "/mount/child", false);
    let top = tree.bind("/storage/top", "/mount", false);
    assert_eq!(
        tree.points
            .iter()
            .map(|point| (point.id, point.parent))
            .collect::<Vec<_>>(),
        [(base, ROOT_MOUNT_ID), (child, base), (top, base)]
    );
    tree.restore();
    assert_eq!(
        tree.translate("/mount/child/file"),
        "/storage/top/child/file"
    );
    assert_eq!(tree.unmount("/mount/child", 0), Err(EINVAL));
    tree.unmount("/mount", 0).unwrap();
    assert_eq!(tree.translate("/mount/child/file"), "/storage/child/file");
    assert_eq!(tree.unmount("/mount", 0), Err(crate::EBUSY));
    tree.unmount("/mount/child", 0).unwrap();
    tree.unmount("/mount", 0).unwrap();
    assert!(tree.points.is_empty());
}

#[test]
fn detach_removes_the_selected_subtree_not_covered_siblings() {
    let mut tree = Tree::new();
    let sibling = tree.bind("/storage/old", "/mount/child", false);
    tree.bind("/storage/base", "/mount", false);
    tree.bind("/storage/new", "/mount/child", false);
    tree.bind("/storage/leaf", "/mount/child/leaf", false);
    assert_eq!(tree.unmount("/mount", 0), Err(crate::EBUSY));
    tree.unmount("/mount", MNT_DETACH).unwrap();
    assert_eq!(tree.points.len(), 1);
    assert_eq!(tree.points[0].id, sibling);
    assert_eq!(tree.translate("/mount/child/file"), "/storage/old/file");
}

#[test]
fn nonrecursive_bind_never_follows_source_overmounts() {
    let mut tree = Tree::new();
    tree.bind("/storage/child", "/source/child", false);
    tree.bind("/source", "/copy", false);
    assert_eq!(tree.translate("/copy/child/file"), "/source/child/file");
    tree.bind("/replacement", "/source", false);
    assert_eq!(
        tree.translate("/source/child/file"),
        "/replacement/child/file"
    );
    assert_eq!(tree.translate("/copy/child/file"), "/source/child/file");
    tree.bind("/copy", "/copy2", false);
    assert_eq!(tree.translate("/copy2/child/file"), "/source/child/file");
}

#[test]
fn recursive_bind_clones_selected_subtree_with_independent_ids() {
    let mut tree = Tree::new();
    tree.bind("/outside", "/elsewhere", false);
    tree.bind("/storage/root", "/source", false);
    tree.bind("/storage/child", "/source/child", false);
    tree.bind("/storage/leaf", "/source/child/leaf", false);
    let copy = tree.bind("/source", "/copy", true);
    assert_eq!(tree.points.len(), 7);
    assert_eq!(tree.points[4].id, copy);
    assert_eq!(tree.points[5].parent, copy);
    assert_eq!(tree.points[6].parent, tree.points[5].id);
    tree.restore();
    assert_eq!(
        tree.translate("/copy/child/leaf/file"),
        "/storage/leaf/file"
    );
    tree.unmount("/source", MNT_DETACH).unwrap();
    assert_eq!(
        tree.translate("/copy/child/leaf/file"),
        "/storage/leaf/file"
    );
    assert_eq!(tree.unmount("/copy", 0), Err(crate::EBUSY));
    tree.unmount("/copy", MNT_DETACH).unwrap();
    assert_eq!(tree.points.len(), 1);
    assert_eq!(tree.translate("/elsewhere/file"), "/outside/file");
}

#[test]
fn recursive_bind_of_subdirectory_excludes_sibling_mounts() {
    let mut tree = Tree::new();
    tree.bind("/storage/root", "/source", false);
    tree.bind("/storage/child", "/source/sub/child", false);
    tree.bind("/storage/other", "/source/other", false);
    let copy = tree.bind("/source/sub", "/copy", true);
    assert_eq!(tree.points.len(), 5);
    assert_eq!(tree.points[4].parent, copy);
    assert_eq!(tree.translate("/copy/child/file"), "/storage/child/file");
    assert_eq!(
        tree.translate("/copy/other/file"),
        "/storage/root/sub/other/file"
    );
}

#[test]
fn recursive_bind_preserves_hidden_descendants_without_exposing_them() {
    let mut tree = Tree::new();
    tree.bind("/storage/root", "/source", false);
    tree.bind("/storage/hidden", "/source/sub/child", false);
    tree.bind("/storage/cover", "/source/sub", false);
    tree.bind("/source", "/copy", true);
    tree.restore();
    assert_eq!(
        tree.translate("/copy/sub/child/file"),
        "/storage/cover/child/file"
    );
    assert_eq!(tree.unmount("/copy/sub/child", 0), Err(EINVAL));
    tree.unmount("/copy/sub", 0).unwrap();
    assert_eq!(
        tree.translate("/copy/sub/child/file"),
        "/storage/hidden/file"
    );
}

#[test]
fn self_bind_and_bind_from_descendant_have_no_translation_cycle() {
    let mut tree = Tree::new();
    tree.bind("/source", "/source", false);
    assert_eq!(tree.translate("/source/file"), "/source/file");
    tree.bind("/source/sub", "/source", false);
    assert_eq!(tree.translate("/source/file"), "/source/sub/file");
    tree.restore();
    tree.unmount("/source", 0).unwrap();
    assert_eq!(tree.translate("/source/file"), "/source/file");
}

#[test]
fn mount_ids_are_not_reused_after_empty_table_or_restore() {
    let mut tree = Tree::new();
    let first = tree.bind("/one", "/mount", false);
    tree.unmount("/mount", 0).unwrap();
    tree.restore();
    let second = tree.bind("/two", "/mount", false);
    assert!(second > first);
    let frozen = tree.points.clone();
    tree.next = MAX_MOUNT_ID + 1;
    assert_eq!(
        bind_in(
            &mut tree.points,
            &mut tree.next,
            "/three",
            "/other",
            MS_BIND
        ),
        Err(crate::EOVERFLOW)
    );
    assert_eq!(tree.points, frozen);
}

#[test]
fn malformed_parent_topologies_and_old_payloads_fail_closed() {
    let mut tree = Tree::new();
    tree.bind("/one", "/mount", false);
    tree.bind("/two", "/mount/sub", false);
    for parent in [tree.points[0].id, u64::MAX] {
        let mut corrupt = tree.points.clone();
        corrupt[0].parent = parent;
        assert_eq!(
            decode_table(&encode_table_with_next_id(&corrupt, tree.next).unwrap()),
            Err(EIO)
        );
    }
    let mut corrupt = tree.points.clone();
    corrupt[1].target = "/outside".into();
    assert_eq!(
        decode_table(&encode_table_with_next_id(&corrupt, tree.next).unwrap()),
        Err(EIO)
    );
    let mut corrupt = tree.points.clone();
    corrupt[1].parent = ROOT_MOUNT_ID;
    corrupt[1].target = "/mount".into();
    assert_eq!(
        decode_table(&encode_table_with_next_id(&corrupt, tree.next).unwrap()),
        Err(EIO)
    );
    let mut old = encode_table_with_next_id(&[], FIRST_DYNAMIC_MOUNT_ID).unwrap();
    old[..8].copy_from_slice(b"CYSMOUNT");
    assert_eq!(decode_table(&old), Err(EIO));
}

#[test]
fn fixed_virtual_trees_and_root_cannot_publish_ineffective_binds() {
    let mut tree = Tree::new();
    for path in [
        "/",
        "/sys",
        "/sys/fs/cgroup",
        "/dev",
        "/dev/null",
        "/dev/shm",
    ] {
        assert_eq!(
            bind_in(&mut tree.points, &mut tree.next, path, "/mount", MS_BIND),
            Err(EOPNOTSUPP)
        );
        assert_eq!(
            bind_in(&mut tree.points, &mut tree.next, "/source", path, MS_BIND),
            Err(EOPNOTSUPP)
        );
        assert!(tree.points.is_empty());
        assert_eq!(tree.next, FIRST_DYNAMIC_MOUNT_ID);
    }
    // Proc mounts now carry their virtual dispatch; they are not native
    // pathname aliases. Keep rejecting native binds over the builtin tree.
    for path in ["/proc", "/proc/self"] {
        let mut proc_tree = Tree::new();
        bind_in(
            &mut proc_tree.points,
            &mut proc_tree.next,
            path,
            "/mount",
            MS_BIND,
        )
        .unwrap();
        assert_ne!(proc_tree.points[0].flags & MS_PROC, 0);
        assert_eq!(translate_in(&proc_tree.points, "/mount"), Err(EOPNOTSUPP));
        assert_eq!(
            bind_in(&mut tree.points, &mut tree.next, "/source", path, MS_BIND),
            Err(EOPNOTSUPP)
        );
        assert!(tree.points.is_empty());
    }
    tree.bind("/proc-fixture", "/device-fixture", false);
    assert_eq!(tree.translate("/device-fixture/file"), "/proc-fixture/file");
}

#[test]
fn propagation_preserves_intermediate_shared_slave_masters() {
    let original = open_namespace(crate::FdFlags::NONE).unwrap();
    unshare_namespace().unwrap();
    let root = std::env::temp_dir().join(format!(
        "kinakaze-propagation-chain-{}-{}",
        std::process::id(),
        shared::next_group().unwrap()
    ));
    for path in ["base/a", "target", "payload/sub"] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    std::fs::write(root.join("payload/value"), b"chain").unwrap();
    let guest = |path: &str| crate::to_guest_path(&root.join(path));
    bind(&guest("base"), &guest("target"), MS_BIND).unwrap();
    set_propagation(&guest("target"), MS_SHARED).unwrap();
    let parent = open_namespace(crate::FdFlags::NONE).unwrap();
    unshare_namespace().unwrap();
    set_propagation(&guest("target"), MS_SLAVE).unwrap();
    set_propagation(&guest("target"), MS_SHARED).unwrap();
    let middle = open_namespace(crate::FdFlags::NONE).unwrap();
    unshare_namespace().unwrap();
    set_propagation(&guest("target"), MS_SLAVE).unwrap();
    let leaf = open_namespace(crate::FdFlags::NONE).unwrap();
    enter_namespace(parent).unwrap();
    bind(&guest("payload"), &guest("target/a"), MS_BIND).unwrap();
    enter_namespace(middle).unwrap();
    let middle_peer = visible_mount(&snapshot().unwrap(), &guest("target/a"))
        .unwrap()
        .unwrap()
        .meta
        .peer;
    assert_ne!(middle_peer, 0);
    bind(&guest("payload"), &guest("target/a/sub"), MS_BIND).unwrap();
    enter_namespace(leaf).unwrap();
    let table = snapshot().unwrap();
    assert_eq!(
        visible_mount(&table, &guest("target/a"))
            .unwrap()
            .unwrap()
            .meta
            .master,
        middle_peer
    );
    assert_eq!(
        translate(&guest("target/a/sub/value")).unwrap(),
        guest("payload/value")
    );
    enter_namespace(parent).unwrap();
    assert_eq!(
        translate(&guest("target/a/sub/value")).unwrap(),
        guest("payload/sub/value")
    );
    unmount(&guest("target"), MNT_DETACH).unwrap();
    for fd in [middle, leaf] {
        enter_namespace(fd).unwrap();
        unmount(&guest("target"), MNT_DETACH).unwrap();
    }
    enter_namespace(original).unwrap();
    for fd in [leaf, middle, parent, original] {
        crate::close(fd).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn namespace_snapshot_does_not_wait_on_a_frozen_parent_topology_writer() {
    let _ = snapshot().unwrap();
    let guard = shared::topology_guard().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        sender.send(snapshot().map(|_| ())).unwrap();
    });
    let result = receiver.recv_timeout(std::time::Duration::from_secs(1));
    drop(guard);
    worker.join().unwrap();
    assert_eq!(result.unwrap(), Ok(()));
}
