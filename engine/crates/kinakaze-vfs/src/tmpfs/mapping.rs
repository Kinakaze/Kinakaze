//! Zero-copy views of tmpfs pages. Kernel object leases outlive descriptors and
//! are duplicated by libc's existing fork mapping transaction.
use super::*;
use crate::fs::object::Object;

pub struct View {
    section: Object,
    lease: Object,
    metadata: Object,
    pub capacity: usize,
    pub identity: [u64; 3],
    pub maximum: i32,
    /// (mapping byte offset, section byte offset, byte length).
    pub runs: Vec<(usize, u64, usize)>,
}

impl View {
    /// Transfers ownership of three non-inheritable native handles to the
    /// managed mapping record. The caller must close or register all of them.
    pub fn into_handles(self) -> [usize; 3] {
        [
            self.section.into_raw() as usize,
            self.lease.into_raw() as usize,
            self.metadata.into_raw() as usize,
        ]
    }
}

pub fn prepare(
    fd: i32,
    offset: u64,
    length: usize,
    shared: bool,
    protection: i32,
) -> Result<View, i32> {
    let (descriptor, location, _, flags) = descriptor(fd)?;
    if flags & O_PATH != 0 {
        return Err(EBADF);
    }
    if flags & O_ACCMODE == O_WRONLY {
        return Err(EACCES);
    }
    if protection & !7 != 0 || offset % PAGE != 0 || length == 0 || length as u64 % PAGE != 0 {
        return Err(EINVAL);
    }
    let end = offset
        .checked_add(length as u64)
        .filter(|end| *end <= i64::MAX as u64)
        .ok_or(EOVERFLOW)?;
    let mount_flags =
        crate::mount::policy::get(location.namespace, location.mount, location.flags)?.flags();
    let writable = !shared || flags & O_ACCMODE == O_RDWR && mount_flags & 1 == 0;
    if shared && protection & 2 != 0 && !writable {
        return Err(EACCES);
    }
    if protection & 4 != 0 && mount_flags & 8 != 0 {
        return Err(EPERM);
    }
    let volume = volume(location.volume)?;
    let lease = shared::new_object()?;
    volume.meta.lease_kernel(lease.kernel_key())?;
    let mut view = View {
        section: Object::duplicate(volume.section)?,
        lease: lease.pin()?,
        metadata: volume.meta.pin()?,
        capacity: usize::try_from(volume.capacity).map_err(|_| EOVERFLOW)?,
        identity: [
            0x544d504653,
            kinakaze_runtime::authority::domain_id(),
            location.volume,
        ],
        maximum: 1 | if writable { 2 } else { 0 } | if mount_flags & 8 == 0 { 4 } else { 0 },
        runs: Vec::new(),
    };
    volume.change(|state| {
        if state.sys.is_some() || state.mq.is_some() || state.pts.is_some() {
            return Err(ENODEV);
        }
        let mut node = state.nodes.get(&location.node).ok_or(ENOENT)?.clone();
        if node.mode & S_IFMT != S_IFREG {
            return Err(ENODEV);
        }
        // Until EOF fault/revocation is integrated, never expose other inodes'
        // pages or manufacture writable anonymous bytes beyond this inode.
        if end > node.size.div_ceil(PAGE) * PAGE {
            return Err(ENODEV);
        }
        for page in offset / PAGE..end / PAGE {
            let slot = if let Some(&slot) = node.pages.get(&page) {
                slot
            } else {
                let slot = volume.allocate(state)?;
                node.pages.insert(page, slot);
                slot
            };
            let delta = (page * PAGE - offset) as usize;
            if let Some((_, previous, bytes)) = view.runs.last_mut()
                && *previous + *bytes as u64 == slot * PAGE
            {
                *bytes += PAGE as usize;
            } else {
                view.runs.push((delta, slot * PAGE, PAGE as usize));
            }
        }
        node.atime = now();
        node.mappings.push((lease.id(), offset / PAGE, end / PAGE));
        state.nodes.insert(location.node, node);
        Ok(())
    })?;
    drop(descriptor);
    Ok(view)
}
