//! Inode ownership retained by file-origin VMAs, independent of fd-table slots.
//!
//! The owner and its handle slot live in the managed fork arena. Fragment
//! metadata is separately cloned so mprotect/split never changes a neighbour's
//! Linux permissions. A copied private page is explicitly unclassified until
//! native COW lineage and a first-write boundary have actually been established.

use super::*;
use core::sync::atomic::AtomicU8;

static PRESENT: AtomicUsize = AtomicUsize::new(0);

pub(super) fn present() -> bool {
    PRESENT.load(Ordering::Acquire) != 0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(super) enum Representation {
    NativeSection = 0,
    VerifiedCache = 1,
    CopiedUnclassified = 2,
}

#[repr(C)]
struct Owner {
    references: AtomicUsize,
    handle: AtomicUsize,
    registered: AtomicUsize,
    mount_writer: [AtomicUsize; 2],
    writer_registered: AtomicUsize,
    may_execute: bool,
    volatile: Option<(u64, u64)>,
    address: AtomicUsize,
    length: usize,
    offset: u64,
    identity: verity::NativeId,
    shared: bool,
    initial_protection: c_int,
}

pub(super) struct FileOriginRef(*mut Owner);
unsafe impl Send for FileOriginRef {}
unsafe impl Sync for FileOriginRef {}

impl FileOriginRef {
    fn layout(length: usize) -> Result<Layout, i32> {
        Layout::from_size_align(
            size_of::<Owner>()
                .checked_add(length / memory_geometry().0)
                .ok_or(ENOMEM)?,
            align_of::<Owner>(),
        )
        .map_err(|_| ENOMEM)
    }

    pub(super) fn new(
        opened: &fs::verity::Opened,
        length: usize,
        offset: u64,
        shared: bool,
        protection: c_int,
    ) -> Result<Self, i32> {
        let length = page_rounded_length(length)?;
        let may_execute = !opened.mount_noexec();
        let volatile = opened.volatile_state()?;
        if !may_execute && protection & PROT_EXEC != 0 {
            return Err(kinakaze_vfs::EPERM);
        }
        let mut mount_writer = opened.mount_writer()?;
        if length == 0 || protection & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
            return Err(EINVAL);
        }
        offset.checked_add(length as u64).ok_or(EOVERFLOW)?;
        let layout = Self::layout(length)?;
        let identity = verity::native_id(opened.borrowed_handle())?;
        let mut duplicate = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                opened.borrowed_handle(),
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(last_errno());
        }
        let raw = unsafe { kinakaze_alloc::ManagedAllocator.alloc(layout) }.cast::<Owner>();
        if raw.is_null() {
            unsafe { CloseHandle(duplicate) };
            return Err(ENOMEM);
        }
        unsafe {
            raw.write(Owner {
                references: AtomicUsize::new(1),
                handle: AtomicUsize::new(duplicate as usize),
                registered: AtomicUsize::new(0),
                mount_writer: std::array::from_fn(|_| {
                    AtomicUsize::new(mount_writer.pop().map_or(0, |w| {
                        use std::os::windows::io::IntoRawHandle;
                        w.into_raw_handle() as usize
                    }))
                }),
                writer_registered: AtomicUsize::new(0),
                may_execute,
                volatile,
                address: AtomicUsize::new(0),
                length,
                offset,
                identity,
                shared,
                initial_protection: protection,
            });
        }
        let result = Self(raw);
        for index in 0..length / memory_geometry().0 {
            unsafe {
                result
                    .protections()
                    .add(index)
                    .write(AtomicU8::new(protection as u8))
            };
        }
        if !unsafe { kinakaze_runtime::register_fork_handle_slot(result.slot()) } {
            return Err(ENOMEM);
        }
        result.owner().registered.store(1, Ordering::Release);
        for index in 0..2 {
            if result.owner().mount_writer[index].load(Ordering::Acquire) != 0 {
                if !unsafe {
                    kinakaze_runtime::register_fork_handle_slot(result.writer_slot(index))
                } {
                    return Err(ENOMEM);
                }
                result
                    .owner()
                    .writer_registered
                    .fetch_or(1 << index, Ordering::Release);
            }
        }
        PRESENT.store(1, Ordering::Release);
        Ok(result)
    }

    fn owner(&self) -> &Owner {
        unsafe { &*self.0 }
    }

    fn slot(&self) -> *const AtomicUsize {
        unsafe { ptr::addr_of!((*self.0).handle) }
    }
    fn writer_slot(&self, index: usize) -> *const AtomicUsize {
        unsafe { ptr::addr_of!((*self.0).mount_writer[index]) }
    }

    fn protections(&self) -> *mut AtomicU8 {
        unsafe { self.0.add(1).cast() }
    }

    pub(super) fn protection(&self, address: usize) -> Result<c_int, i32> {
        let offset = address.checked_sub(self.address()).ok_or(EINVAL)?;
        if offset >= self.length() {
            return Err(EINVAL);
        }
        Ok(unsafe {
            (*self.protections().add(offset / memory_geometry().0)).load(Ordering::Acquire)
        } as c_int)
    }

    pub(super) fn set_protection(&self, address: usize, protection: c_int) -> Result<(), i32> {
        let offset = address.checked_sub(self.address()).ok_or(EINVAL)?;
        if offset >= self.length() {
            return Err(EINVAL);
        }
        unsafe {
            (*self.protections().add(offset / memory_geometry().0))
                .store(protection as u8, Ordering::Release)
        };
        Ok(())
    }

    pub(super) fn bind(&self, address: usize) -> Result<(), i32> {
        if address == 0 || address.checked_add(self.owner().length).is_none() {
            return Err(EINVAL);
        }
        self.owner()
            .address
            .compare_exchange(0, address, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| EINVAL)
    }

    pub(super) fn volatile(&self) -> Option<(u64, u64)> {
        self.owner().volatile
    }

    pub(super) fn handle(&self) -> *mut c_void {
        self.owner().handle.load(Ordering::Acquire) as *mut c_void
    }

    pub(super) fn identity(&self) -> verity::NativeId {
        self.owner().identity
    }

    pub(super) fn address(&self) -> usize {
        self.owner().address.load(Ordering::Acquire)
    }

    pub(super) fn length(&self) -> usize {
        self.owner().length
    }

    pub(super) fn offset(&self, address: usize) -> Result<u64, i32> {
        let delta = address.checked_sub(self.address()).ok_or(EINVAL)?;
        if delta > self.length() {
            return Err(EINVAL);
        }
        self.owner()
            .offset
            .checked_add(delta as u64)
            .ok_or(EOVERFLOW)
    }

    pub(super) fn shared(&self) -> bool {
        self.owner().shared
    }

    #[cfg(test)]
    pub(super) fn initial_protection(&self) -> c_int {
        self.owner().initial_protection
    }

    unsafe fn adopt(raw: usize) -> Result<Self, i32> {
        let end = kinakaze_alloc::ARENA_BASE + kinakaze_alloc::ARENA_SIZE;
        if raw < kinakaze_alloc::ARENA_BASE
            || raw % align_of::<Owner>() != 0
            || raw
                .checked_add(size_of::<Owner>())
                .is_none_or(|at| at > end)
        {
            return Err(EINVAL);
        }
        let owner = unsafe { &*(raw as *const Owner) };
        let address = owner.address.load(Ordering::Acquire);
        if owner.references.load(Ordering::Acquire) == 0
            || owner.registered.load(Ordering::Acquire) != 1
            || address == 0
            || owner.length == 0
            || address.checked_add(owner.length).is_none()
            || owner.length % memory_geometry().0 != 0
            || raw
                .checked_add(Self::layout(owner.length)?.size())
                .is_none_or(|at| at > end)
            || owner.initial_protection & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0
        {
            return Err(EINVAL);
        }
        // The runtime duplicates and patches this slot before any provider
        // child callback; a copied parent handle value is never adopted blindly.
        let handle = owner.handle.load(Ordering::Acquire) as *mut c_void;
        if handle.is_null() || verity::native_id(handle)? != owner.identity {
            return Err(EIO);
        }
        PRESENT.store(1, Ordering::Release);
        Ok(Self(raw as *mut Owner))
    }
}

impl Clone for FileOriginRef {
    fn clone(&self) -> Self {
        self.owner().references.fetch_add(1, Ordering::Relaxed);
        Self(self.0)
    }
}

impl Drop for FileOriginRef {
    fn drop(&mut self) {
        // Mapping transactions are recursive for the current host thread.
        // Final retirement and slot removal must be indivisible with fork.
        let Some(_transaction) = kinakaze_runtime::begin_fork_mapping_transaction() else {
            return; // A poisoned coordinator must not publish a stale closed slot.
        };
        if self.owner().references.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        if self.owner().registered.load(Ordering::Acquire) != 0
            && !kinakaze_runtime::unregister_fork_handle_slot(self.slot())
        {
            self.owner().references.store(1, Ordering::Release);
            return; // Never free a slot still reachable by the fork coordinator.
        }
        let handle = self.owner().handle.swap(0, Ordering::AcqRel) as *mut c_void;
        for index in 0..2 {
            if self.owner().writer_registered.load(Ordering::Acquire) & (1 << index) != 0
                && !kinakaze_runtime::unregister_fork_handle_slot(self.writer_slot(index))
            {
                // Keep storage and the still-registered native writer slot alive.
                self.owner().references.store(1, Ordering::Release);
                self.owner()
                    .handle
                    .store(handle as usize, Ordering::Release);
                self.owner().registered.store(0, Ordering::Release);
                return;
            }
            self.owner()
                .writer_registered
                .fetch_and(!(1 << index), Ordering::Release);
        }
        for slot in &self.owner().mount_writer {
            let writer = slot.swap(0, Ordering::AcqRel) as *mut c_void;
            if !writer.is_null() {
                unsafe { CloseHandle(writer) };
            }
        }
        if !handle.is_null() {
            unsafe { CloseHandle(handle) };
        }
        let layout = Self::layout(self.length()).unwrap();
        unsafe {
            ptr::drop_in_place(self.0);
            kinakaze_alloc::ManagedAllocator.dealloc(self.0.cast(), layout);
        }
    }
}

#[derive(Clone)]
pub(super) struct Info {
    pub(super) owner: FileOriginRef,
    pub(super) representation: Representation,
}

pub(super) fn encode(info: Option<&Info>, output: &mut [u8]) {
    output.fill(0);
    if let Some(info) = info {
        output[..8].copy_from_slice(&(info.owner.0 as usize as u64).to_le_bytes());
        output[12..16].copy_from_slice(&(info.representation as u32).to_le_bytes());
    }
}

pub(super) unsafe fn decode(
    input: &[u8],
    start: usize,
    length: usize,
) -> Result<Option<Info>, i32> {
    let raw = u64::from_le_bytes(input[..8].try_into().unwrap()) as usize;
    if raw == 0 {
        return if input.iter().all(|&v| v == 0) {
            Ok(None)
        } else {
            Err(EINVAL)
        };
    }
    let representation = match u32::from_le_bytes(input[12..16].try_into().unwrap()) {
        0 => Representation::NativeSection,
        1 => Representation::VerifiedCache,
        2 => Representation::CopiedUnclassified,
        _ => return Err(EINVAL),
    };
    if input[8..12] != [0; 4] {
        return Err(EINVAL);
    }
    let owner = unsafe { FileOriginRef::adopt(raw)? };
    if start < owner.address()
        || start
            .checked_add(length)
            .is_none_or(|end| end > owner.address().saturating_add(owner.length()))
    {
        return Err(EINVAL);
    }
    Ok(Some(Info {
        owner,
        representation,
    }))
}

/// Instantiate only the now-accessible part of a retained EOF reservation.
/// This uses the inode handle, never the original fd or its former pathname.
fn materialize_placeholder(
    owner: &FileOriginRef,
    start: usize,
    length: usize,
    protection: c_int,
) -> Result<(), i32> {
    if protection == PROT_NONE {
        return Ok(());
    }
    let section_protection = if owner.shared() {
        shared_section_protection(owner.handle(), protection)?
    } else {
        section_protection(protection, true)
    };
    let _inode = fs::verity::lock(owner.handle())?;
    let offset = owner.offset(start)?;
    let eof = fs::verity::authoritative_size(owner.handle())?;
    let length = page_rounded_length(eof.saturating_sub(offset).min(length as u64) as usize)?;
    if length == 0 {
        return Ok(());
    }
    let view_protection = page_protection(protection, !owner.shared())?;
    let section = unsafe {
        CreateFileMappingW(
            owner.handle(),
            ptr::null(),
            section_protection,
            0,
            0,
            ptr::null(),
        )
    };
    if section.is_null() {
        return Err(last_errno());
    }
    let retained = if owner.shared() {
        match BackingRef::new_retained_shared(
            section,
            page_rounded_length(eof as usize)?,
            section_protection,
        ) {
            Ok(backing) => Some(backing),
            Err(error) => {
                unsafe { CloseHandle(section) };
                return Err(error);
            }
        }
    } else {
        None
    };
    let result = replace_placeholder_view(
        section,
        start as *mut c_void,
        length,
        offset,
        view_protection,
        owner.shared(),
        retained.as_ref(),
    );
    if retained.is_none() {
        unsafe { CloseHandle(section) };
    }
    match result? {
        Some(mapped) if mapped as usize == start => Ok(()),
        _ => Err(EIO),
    }
}

/// Apply Linux protection to retained file pages without losing a private
/// section's WRITECOPY contract or allocating ordinary anonymous pages at EOF.
/// Per-page metadata avoids physically remapping a native COW view just to
/// represent a partial mprotect; such remapping would destroy its dirty pages.
pub(super) fn mprotect(start: usize, length: usize, protection: c_int) -> Result<bool, i32> {
    let page = memory_geometry().0;
    if start % page != 0 || protection & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
        return Err(EINVAL);
    }
    let end = start
        .checked_add(page_rounded_length(length)?)
        .ok_or(EINVAL)?;
    let mut runs = {
        let registry = mappings().lock().map_err(|_| EIO)?;
        registry
            .iter()
            .filter_map(|(&at, mapping)| {
                let stop = at.saturating_add(mapping.length).min(end);
                (at < end && start < stop).then(|| (at.max(start), stop, mapping.clone()))
            })
            .collect::<Vec<_>>()
    };
    if !runs
        .iter()
        .any(|(_, _, mapping)| mapping.file_origin.is_some())
    {
        return Ok(false);
    }
    validate_madvise_range(start, end, false)?;
    if protection & PROT_EXEC != 0
        && runs.iter().any(|(_, _, mapping)| {
            mapping
                .file_origin
                .as_ref()
                .is_some_and(|info| !info.owner.owner().may_execute)
        })
    {
        return Err(kinakaze_vfs::EACCES);
    }
    runs.sort_unstable_by_key(|run| run.0);
    for (from, to, mapping) in runs {
        let Some(info) = mapping.file_origin else {
            if unsafe { mprotect_native_raw(from as *mut c_void, to - from, protection) } != 0 {
                return Err(crate::kinakaze_errno());
            }
            continue;
        };
        if info.owner.shared()
            && protection & PROT_WRITE != 0
            && !handle_access(info.owner.handle())
                .is_some_and(|access| access & FILE_WRITE_DATA != 0)
        {
            return Err(kinakaze_vfs::EACCES);
        }
        if mapping.verity.is_some() {
            verity::protect(from, to, protection)?;
        } else if mapping.kind == MappingKind::Placeholder {
            materialize_placeholder(&info.owner, from, to - from, protection)?;
        } else {
            let native =
                !info.owner.shared() && info.representation == Representation::NativeSection;
            let win_protection = page_protection(protection, native)?;
            let mut ignored = 0;
            if unsafe {
                VirtualProtect(from as *mut c_void, to - from, win_protection, &mut ignored)
            } == 0
            {
                return Err(last_errno());
            }
        }
        for at in (from..to).step_by(page) {
            info.owner.set_protection(at, protection)?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
