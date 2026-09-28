//! Materialize the initial guest namespace's builtins as real attachments.
//! This is independent of the guest's choice of PID 1. Namespace clones retain
//! the attachments through the ordinary mount table and init object custody.
use super::*;

pub fn initialize_builtin_mounts() -> Result<(), i32> {
    if kinakaze_runtime::authority::get().is_none() || !snapshot()?.is_empty() {
        return Ok(());
    }
    let proc = crate::procfs::instance::prepare("")?;
    let sys = crate::sysfs::prepare("")?;
    let cgroup = crate::tmpfs::cgroupfs::prepare("")?;
    let dev = crate::tmpfs::prepare_devices()?;
    let pts = crate::devpts::prepare("mode=0620,ptmxmode=0666")?;
    let shm = crate::tmpfs::prepare("mode=1777")?;
    let mq = crate::mqueue::prepare("")?;
    update(|table, next| {
        // A second first-launch worker may have completed initialization while
        // the backends above were being prepared. Publish the tree only once.
        if !table.is_empty() {
            return Ok(());
        }
        table.push(MountPoint {
            meta: MountMetadata::default(),
            id: ROOT_MOUNT_ID,
            parent: 0,
            source: "/".into(),
            target: "/".into(),
            flags: MS_ROOT,
        });
        for (target, source, flags) in [
            ("/proc", proc, MS_PROC | 2 | 4 | 8),
            ("/sys", sys, MS_TMPFS | MS_SYSFS | 2 | 4 | 8),
            ("/sys/fs/cgroup", cgroup, MS_TMPFS | MS_CGROUP | 2 | 4 | 8),
            ("/dev", dev, MS_TMPFS | 2),
            ("/dev/pts", pts, MS_TMPFS | MS_DEVPTS | 2 | 8),
            ("/dev/shm", shm, MS_TMPFS | 2 | 4),
            ("/dev/mqueue", mq, MS_TMPFS | MS_MQUEUE | 2 | 4 | 8),
        ] {
            let parent = visible_mount(table, target)?.map_or(ROOT_MOUNT_ID, |p| p.id);
            table.push(MountPoint {
                meta: MountMetadata::default(),
                id: allocate_id(next)?,
                parent,
                source,
                target: target.into(),
                flags: flags | (1 << 21),
            });
        }
        Ok(())
    })
}
