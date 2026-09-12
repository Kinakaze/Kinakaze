//! Mount setns installs the target namespace's root as both root and cwd.
//! Fork restoration reopens authenticated membership and restores its saved fs_struct.
use super::shared;

pub fn enter_namespace(fd: i32) -> Result<(), i32> {
    let id = shared::descriptor_id(fd)?;
    if !crate::fs_context::is_private() {
        return Err(crate::EINVAL);
    }
    let user = crate::user_namespace::id(crate::job::process_id())?;
    if !crate::user_namespace::capable(user, 18) || !crate::user_namespace::capable(user, 21) {
        return Err(crate::EPERM); // CAP_SYS_CHROOT and CAP_SYS_ADMIN.
    }
    let target = shared::prepare_enter(id)?;
    let previous_mounts = shared::get()?;
    let previous_fs = crate::fs_context::read(Clone::clone);
    // Resolve the new root with the calling task's staged topology. Publish
    // process membership only after both directory references are installed.
    shared::inherit(target.clone());
    crate::fs_context::update(|state| {
        state.root = None;
        state.overlay = None;
        state.cwd = None;
        state.cwd_object = None;
        state.confined = true;
    });
    let result = (|| {
        crate::fs::chroot("/")?;
        crate::fs::chdir("/")?;
        shared::install(target)
    })();
    if result.is_err() {
        shared::inherit(previous_mounts);
        crate::fs_context::update(|state| *state = previous_fs);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mount::{self, MountMetadata, MountPoint};

    #[test]
    fn failed_root_lookup_keeps_namespace_and_fs_state() {
        crate::fs_context::unshare();
        let original = shared::get().unwrap();
        let saved_fs = crate::fs_context::read(Clone::clone);
        mount::unshare_namespace().unwrap();
        let target = shared::get().unwrap();
        let missing = format!("/missing-setns-root-{}-{}", std::process::id(), target.id());
        let table = vec![MountPoint {
            meta: MountMetadata {
                pivot: mount::FIRST_DYNAMIC_MOUNT_ID,
                ..Default::default()
            },
            id: mount::FIRST_DYNAMIC_MOUNT_ID,
            parent: mount::ROOT_MOUNT_ID,
            source: missing,
            target: "/target-root".into(),
            flags: mount::MS_BIND,
        }];
        target
            .update(|_| Ok((mount::encode_table(&table)?, ())))
            .unwrap();
        let fd = mount::open_namespace(crate::FdFlags::NONE).unwrap();
        shared::enter(original.id()).unwrap();
        crate::fs::chdir("/").unwrap();
        let before = crate::fs_context::read(Clone::clone);
        let membership = kinakaze_runtime::job::mount_namespace(crate::job::process_id());

        assert_eq!(enter_namespace(fd), Err(crate::ENOENT));
        assert_eq!(shared::get().unwrap().id(), original.id());
        assert_eq!(
            kinakaze_runtime::job::mount_namespace(crate::job::process_id()),
            membership
        );
        crate::fs_context::read(|after| {
            assert_eq!(after.root, before.root);
            assert_eq!(after.cwd, before.cwd);
            assert_eq!(after.confined, before.confined);
            assert_eq!(after.umask, before.umask);
            assert!(after.overlay == before.overlay);
            assert!(std::sync::Arc::ptr_eq(
                after.cwd_object.as_ref().unwrap(),
                before.cwd_object.as_ref().unwrap()
            ));
        });
        crate::close(fd).unwrap();
        crate::fs_context::update(|state| *state = saved_fs);
    }
}
