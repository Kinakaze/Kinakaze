//! Pagefile-backed tmpfs uses the same VMA splitting and fork transport as other
//! retained sections. No staging file, copied MAP_SHARED data, or private fd is
//! needed; one backing owns the lease for every fragment of this mapping.
use super::*;
use kinakaze_vfs::tmpfs::mapping as inode;
use std::collections::BTreeSet;

#[derive(Clone)]
pub(super) struct Info {
    owner: BackingRef,
    volume: u64,
    node: u64,
    lease: u64,
    origin: usize,
    file_offset: u64,
    epoch: u64,
    protection: c_int,
    maximum: c_int,
}
impl Info {
    fn offset(&self, address: usize) -> Result<u64, i32> {
        self.file_offset
            .checked_add(address.checked_sub(self.origin).ok_or(EIO)? as u64)
            .ok_or(EOVERFLOW)
    }
}
static LEASES: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());
static PRESENT: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static FORK_GUARD: std::cell::RefCell<Option<inode::Transaction>> = const { std::cell::RefCell::new(None) };
}
pub(super) unsafe extern "system" fn fork_prepare() -> i32 {
    match inode::transaction() {
        Ok(guard) => {
            FORK_GUARD.with(|slot| *slot.borrow_mut() = Some(guard));
            0
        }
        Err(error) => error,
    }
}
pub(super) unsafe extern "system" fn fork_parent(_: i32) {
    FORK_GUARD.with(|slot| slot.borrow_mut().take());
}

pub(super) fn encode(info: Option<&Info>, output: &mut [u8]) {
    output.fill(0);
    if let Some(info) = info {
        for (slot, value) in output[..56].chunks_exact_mut(8).zip([
            info.owner.raw() as u64,
            info.volume,
            info.node,
            info.lease,
            info.origin as u64,
            info.file_offset,
            info.epoch,
        ]) {
            slot.copy_from_slice(&value.to_le_bytes());
        }
        output[56..60].copy_from_slice(&info.protection.to_le_bytes());
        output[60..64].copy_from_slice(&info.maximum.to_le_bytes());
    }
}
pub(super) unsafe fn decode(bytes: &[u8], start: usize) -> Result<Option<Info>, i32> {
    let words: Vec<u64> = bytes[..56]
        .chunks_exact(8)
        .map(|part| u64::from_le_bytes(part.try_into().unwrap()))
        .collect();
    if words[0] == 0 {
        return Ok(None);
    }
    let info = Info {
        owner: unsafe { BackingRef::adopt(words[0] as usize)? },
        volume: words[1],
        node: words[2],
        lease: words[3],
        origin: words[4] as usize,
        file_offset: words[5],
        epoch: words[6],
        protection: i32::from_le_bytes(bytes[56..60].try_into().unwrap()),
        maximum: i32::from_le_bytes(bytes[60..64].try_into().unwrap()),
    };
    info.offset(start)?;
    Ok(Some(info))
}

pub(super) fn map(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    shared: bool,
    fixed: bool,
    fd: i32,
    offset: u64,
) -> Result<*mut c_void, i32> {
    PRESENT.store(1, Ordering::Release);
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;
    let length = page_rounded_length(length)?;
    let view = kinakaze_vfs::tmpfs::mapping::prepare(fd, offset, length, shared, protection)?;
    let maximum = page_protection(view.maximum, !shared)?;
    let current = page_protection(protection, !shared)?;
    let capacity = view.capacity;
    let identity = view.identity;
    let runs = view.runs.clone();
    let identity_data = (
        view.volume,
        view.node,
        view.lease_id,
        view.epoch,
        view.maximum,
    );
    let [section, lease, metadata] = view.into_handles();
    let backing =
        match BackingRef::new_retained(section as _, capacity, maximum, if shared { 1 } else { 2 })
        {
            Ok(backing) => backing,
            Err(error) => {
                for handle in [section, lease, metadata] {
                    unsafe { CloseHandle(handle as _) };
                }
                return Err(error);
            }
        };
    unsafe {
        (*backing.0).futex_identity = identity;
        (*backing.0).pins[0].store(lease, Ordering::Release);
        (*backing.0).pins[1].store(metadata, Ordering::Release);
    }
    for index in 0..2 {
        if !unsafe {
            kinakaze_runtime::register_fork_handle_slot(ptr::addr_of!((*backing.0).pins[index]))
        } {
            return Err(ENOMEM);
        }
        unsafe {
            (*backing.0)
                .registered_pins
                .fetch_or(1 << index, Ordering::Release)
        };
    }
    let mapped = map_anonymous(address, length, PROT_NONE, false, fixed)?;
    let info = Info {
        owner: backing.clone(),
        volume: identity_data.0,
        node: identity_data.1,
        lease: identity_data.2,
        epoch: identity_data.3,
        maximum: identity_data.4,
        origin: mapped as usize,
        file_offset: offset,
        protection,
    };
    {
        let mut registry = mappings().lock().map_err(|_| EIO)?;
        let mapping = registry.get_mut(&(mapped as usize)).ok_or(EIO)?;
        mapping.tmpfs = Some(info);
        mapping.shared = shared;
        register_fork_fragment(mapped as usize, mapping);
    }
    let result = (|| {
        for (delta, section_offset, bytes) in runs {
            let at = unsafe { mapped.byte_add(delta) };
            replace_placeholder_view(
                section as _,
                at,
                bytes,
                section_offset,
                maximum,
                shared,
                Some(&backing),
            )?
            .ok_or(ENOMEM)?;
            let mut old = 0;
            if current != maximum && unsafe { VirtualProtect(at, bytes, current, &mut old) } == 0 {
                return Err(last_errno());
            }
        }
        Ok(mapped)
    })();
    if result.is_err() {
        let _ = unmap(mapped, length);
    }
    result
}

/// Enrollment is updated while the caller still holds the cross-process
/// transaction, so an address cannot be recycled under a stale remote record.
pub(super) fn refresh() -> Result<(), i32> {
    if PRESENT.load(Ordering::Acquire) == 0 {
        return Ok(());
    }
    let registry = mappings().lock().map_err(|_| EIO)?;
    let mut current = std::collections::BTreeMap::<u64, Vec<(usize, usize, u64)>>::new();
    for (&start, mapping) in registry.iter() {
        if let Some(info) = &mapping.tmpfs {
            current.entry(info.lease).or_default().push((
                start,
                mapping.length,
                info.offset(start)?,
            ));
        }
    }
    let mut previous = LEASES.lock().map_err(|_| EIO)?;
    for lease in previous.iter().filter(|id| !current.contains_key(id)) {
        match inode::publish(*lease, &[]) {
            Ok(()) | Err(kinakaze_vfs::ENOENT) => (),
            Err(error) => return Err(error),
        }
    }
    for (&lease, ranges) in &current {
        inode::publish(lease, ranges)?;
    }
    *previous = current.keys().copied().collect();
    fault::publish(&registry);
    Ok(())
}

pub(super) fn restore() -> Result<(), i32> {
    let _guard = inode::transaction()?;
    let registry = mappings().lock().map_err(|_| EIO)?;
    if registry.values().any(|mapping| mapping.tmpfs.is_some()) {
        PRESENT.store(1, Ordering::Release);
    }
    for (&start, mapping) in registry.iter() {
        let Some(info) = &mapping.tmpfs else {
            continue;
        };
        if mapping.kind == MappingKind::Placeholder {
            continue;
        }
        for address in (start..start + mapping.length).step_by(memory_geometry().0) {
            let page = inode::page(
                info.volume,
                info.node,
                info.offset(address)?,
                info.epoch,
                false,
            )?;
            if page.invalidated {
                let mut old = 0;
                if unsafe {
                    VirtualProtect(address as _, memory_geometry().0, PAGE_NOACCESS, &mut old)
                } == 0
                {
                    return Err(last_errno());
                }
            }
        }
    }
    drop(registry);
    refresh()
}

pub(super) unsafe extern "system" fn prepare_buffer(
    address: usize,
    length: usize,
    writing: bool,
) -> i32 {
    let operation = || -> Result<(), i32> {
        if PRESENT.load(Ordering::Acquire) == 0 {
            return Ok(());
        }
        let _guard = inode::transaction()?;
        let end = address.checked_add(length).ok_or(EFAULT)?;
        let ranges: Vec<_> = {
            let registry = mappings().lock().map_err(|_| EIO)?;
            registry
                .iter()
                .filter_map(|(&start, mapping)| {
                    let stop = (start + mapping.length).min(end);
                    let begin = start.max(address);
                    (begin < stop && mapping.tmpfs.is_some()).then_some((begin, stop))
                })
                .collect()
        };
        for (start, stop) in ranges {
            let mut at = start;
            while at < stop {
                let mut page: MemoryBasicInformation = unsafe { std::mem::zeroed() };
                if unsafe { VirtualQuery(at as _, &mut page, size_of::<MemoryBasicInformation>()) }
                    == 0
                {
                    return Err(EFAULT);
                }
                let accessible = page.state == MEM_COMMIT_STATE
                    && if writing {
                        matches!(
                            page.protect & 0xff,
                            PAGE_READWRITE
                                | PAGE_WRITECOPY
                                | PAGE_EXECUTE_READWRITE
                                | PAGE_EXECUTE_WRITECOPY
                        )
                    } else {
                        page.protect & 0xff != PAGE_NOACCESS && page.protect & 0xff != 0
                    };
                if !accessible && resolve(at, usize::from(writing)) != Ok(-1) {
                    return Err(EFAULT);
                }
                at = (at | (memory_geometry().0 - 1))
                    .checked_add(1)
                    .ok_or(EFAULT)?;
            }
        }
        Ok(())
    };
    operation().err().unwrap_or(0)
}

fn resolve(address: usize, access: usize) -> Result<i32, i32> {
    let _guard = inode::transaction()?;
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;
    let (start, mapping) = {
        let registry = mappings().lock().map_err(|_| EIO)?;
        let Some((&start, mapping)) = registry.iter().find(|(start, mapping)| {
            address >= **start && address - **start < mapping.length && mapping.tmpfs.is_some()
        }) else {
            return Ok(0);
        };
        (start, mapping.clone())
    };
    let mut info = mapping.tmpfs.clone().ok_or(EIO)?;
    if info.protection == PROT_NONE
        || access == 1 && info.protection & PROT_WRITE == 0
        || access == 8 && info.protection & PROT_EXEC == 0
    {
        return Ok(11);
    }
    let address = address & !(memory_geometry().0 - 1);
    let page = inode::page(
        info.volume,
        info.node,
        info.offset(address)?,
        info.epoch,
        true,
    )?;
    let Some(slot) = page.slot else {
        return Ok(7);
    };
    let current = page_protection(info.protection, !mapping.shared)?;
    if mapping.kind != MappingKind::Placeholder && !page.invalidated {
        // Revocation may have preceded a failed truncate. Preserve private
        // dirty bytes when the inode epoch proves the original page is live.
        let mut old = 0;
        if unsafe { VirtualProtect(address as _, memory_geometry().0, current, &mut old) } == 0 {
            return Err(last_errno());
        }
        return Ok(-1);
    }
    let _ = start;
    replace_placeholder_view(
        info.owner.handle(),
        address as _,
        memory_geometry().0,
        slot * memory_geometry().0 as u64,
        info.owner.maximum_protection(),
        mapping.shared,
        Some(&info.owner),
    )?
    .ok_or(ENOMEM)?;
    let mut old = 0;
    if unsafe { VirtualProtect(address as _, memory_geometry().0, current, &mut old) } == 0 {
        return Err(last_errno());
    }
    info.epoch = page.epoch;
    mappings()
        .lock()
        .map_err(|_| EIO)?
        .get_mut(&address)
        .ok_or(EIO)?
        .tmpfs = Some(info);
    refresh()?;
    Ok(-1)
}

pub(super) fn mprotect(address: usize, length: usize, protection: c_int) -> Result<bool, i32> {
    if PRESENT.load(Ordering::Acquire) == 0 {
        return Ok(false);
    }
    if address % memory_geometry().0 != 0 || protection & !7 != 0 {
        return Err(EINVAL);
    }
    let length = page_rounded_length(length)?;
    let end = address.checked_add(length).ok_or(EINVAL)?;
    let ranges: Vec<_> = {
        let registry = mappings().lock().map_err(|_| EIO)?;
        registry
            .iter()
            .filter(|(start, mapping)| **start < end && address < **start + mapping.length)
            .map(|(&start, mapping)| (start, mapping.clone()))
            .collect()
    };
    if !ranges.iter().any(|(_, mapping)| mapping.tmpfs.is_some()) {
        return Ok(false);
    }
    let mut cursor = address;
    let mut ranges = ranges;
    ranges.sort_by_key(|(start, _)| *start);
    for (start, mapping) in ranges {
        let from = start.max(address);
        let to = (start + mapping.length).min(end);
        if from != cursor {
            return Err(ENOMEM);
        }
        if let Some(info) = &mapping.tmpfs {
            if protection & !info.maximum != 0 {
                return Err(kinakaze_vfs::EACCES);
            }
            for at in (from..to).step_by(memory_geometry().0) {
                let mut registry = mappings().lock().map_err(|_| EIO)?;
                split_tmpfs_page(&mut registry, at)?;
                let fragment = registry.get_mut(&at).ok_or(EIO)?;
                fragment.tmpfs.as_mut().ok_or(EIO)?.protection = protection;
                drop(registry);
                let state =
                    inode::page(info.volume, info.node, info.offset(at)?, info.epoch, false)?;
                if mapping.kind != MappingKind::Placeholder {
                    let native = if state.invalidated || protection == PROT_NONE {
                        PAGE_NOACCESS
                    } else {
                        page_protection(protection, !mapping.shared)?
                    };
                    let mut old = 0;
                    if unsafe { VirtualProtect(at as _, memory_geometry().0, native, &mut old) }
                        == 0
                    {
                        return Err(last_errno());
                    }
                }
            }
        } else if unsafe { mprotect_native_raw(from as _, to - from, protection) } != 0 {
            return Err(crate::kinakaze_errno());
        }
        cursor = to;
    }
    if cursor != end {
        return Err(ENOMEM);
    }
    refresh()?;
    Ok(true)
}

// Split registry geometry only: native view allocation ownership stays at its
// original base. Placeholder splitting must also split Windows reservations.
fn split_tmpfs_page(registry: &mut HashMap<usize, Mapping>, address: usize) -> Result<(), i32> {
    let length = memory_geometry().0;
    let (&start, mapping) = registry
        .iter()
        .find(|(start, m)| address >= **start && address - **start < m.length)
        .ok_or(EIO)?;
    let mapping = mapping.clone();
    if mapping.kind == MappingKind::Placeholder {
        carve_placeholder_locked(registry, address, length)?;
        return Ok(());
    }
    // Reuse the existing exact-fragment replacement logic to preserve COW bytes
    // and neighbouring permissions while splitting a retained Windows view.
    if start == address && mapping.length == length {
        return Ok(());
    }
    let prepared = prepare_placeholder_locked(registry, address, length)?.ok_or(EIO)?;
    if let Some(previous) = prepared.previous {
        restore_previous_locked(registry, previous)?;
    }
    Ok(())
}

mod fault;
pub(super) use fault::classify;
