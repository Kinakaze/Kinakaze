//! Zero-copy views of tmpfs pages. Kernel object leases outlive descriptors and
//! are duplicated by libc's existing fork mapping transaction.
use super::*;
use crate::fs::object::Object;
mod coordination;
pub(super) use coordination::revoke;
static BUFFER_RESOLVER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub type BufferResolver = unsafe extern "system" fn(usize, usize, bool) -> i32;
pub fn set_buffer_resolver(callback: BufferResolver) {
    BUFFER_RESOLVER.store(callback as usize, std::sync::atomic::Ordering::Release);
}
pub(crate) fn prepare_buffer(address: usize, length: usize, writing: bool) -> Result<(), i32> {
    let callback = BUFFER_RESOLVER.load(std::sync::atomic::Ordering::Acquire);
    if callback == 0 || length == 0 {
        return Ok(());
    }
    let callback: BufferResolver = unsafe { std::mem::transmute(callback) };
    match unsafe { callback(address, length, writing) } {
        0 => Ok(()),
        error => Err(error),
    }
}
pub use coordination::{Transaction, publish, transaction};

pub struct View {
    section: Object,
    lease: Object,
    metadata: Object,
    pub capacity: usize,
    pub identity: [u64; 3],
    pub maximum: i32,
    pub volume: u64,
    pub node: u64,
    pub lease_id: u64,
    pub epoch: u64,
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
    let _transaction = transaction()?;
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
        volume: location.volume,
        node: location.node,
        lease_id: lease.id(),
        epoch: 0,
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
        view.epoch = node.mapping_epoch;
        let backed_end = end.min(node.size.div_ceil(PAGE) * PAGE);
        // Mapping sparse or beyond-EOF pages reserves address space only.
        // Faults allocate real inode pages, so mmap itself cannot consume the
        // tmpfs quota merely because a large range was requested.
        for (&page, &slot) in node
            .pages
            .range(offset / PAGE..backed_end.max(offset) / PAGE)
        {
            let delta = (page * PAGE - offset) as usize;
            if let Some((start, previous, bytes)) = view.runs.last_mut()
                && *previous + *bytes as u64 == slot * PAGE
                && *start + *bytes == delta
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

pub struct Page {
    pub epoch: u64,
    pub invalidated: bool,
    pub slot: Option<u64>,
}

/// Resolve by retained inode identity, never a possibly closed/reused fd.
pub fn page(
    volume_id: u64,
    node_id: u64,
    offset: u64,
    epoch: u64,
    allocate: bool,
) -> Result<Page, i32> {
    let _transaction = transaction()?;
    let volume = volume(volume_id)?;
    let current = volume.decoded()?;
    let node = current.nodes.get(&node_id).ok_or(ENOENT)?;
    let index = offset / PAGE;
    let invalidated = node
        .invalidations
        .range(..=index)
        .next_back()
        .is_some_and(|(_, revision)| *revision > epoch);
    let slot = if index < node.size.div_ceil(PAGE) {
        node.pages.get(&index).copied()
    } else {
        None
    };
    if !allocate || slot.is_some() || index >= node.size.div_ceil(PAGE) {
        return Ok(Page {
            epoch: node.mapping_epoch,
            invalidated,
            slot,
        });
    }
    drop(current);
    volume.change(|state| {
        let mut node = state.nodes.get(&node_id).ok_or(ENOENT)?.clone();
        let index = offset / PAGE;
        let invalidated = node
            .invalidations
            .range(..=index)
            .next_back()
            .is_some_and(|(_, revision)| *revision > epoch);
        let mut slot = if index < node.size.div_ceil(PAGE) {
            node.pages.get(&index).copied()
        } else {
            None
        };
        if slot.is_none() && allocate && index < node.size.div_ceil(PAGE) {
            let allocated = volume.allocate(state)?;
            node.pages.insert(index, allocated);
            slot = Some(allocated);
            state.nodes.insert(node_id, node.clone());
        }
        Ok(Page {
            epoch: node.mapping_epoch,
            invalidated,
            slot,
        })
    })
}
