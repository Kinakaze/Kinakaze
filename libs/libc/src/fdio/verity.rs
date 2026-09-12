//! Immutable verified page-cache snapshots for fs-verity VMAs.
//!
//! Native file sections can fault fresh, unverified storage into memory after
//! mmap returns. Only verified bytes are copied into the guest mapping here.
//! Clean bytes and validity bits live in the managed fork arena, with no new
//! process-local handle registry. They also restore private MADV_DONTNEED pages.

use super::*;

// This protocol is intentionally not wired into ENABLE yet: a participant may
// acknowledge PARKED only after the runtime has established guest quiescence.
// Its independently tested kernel-object lifetime/epoch machinery is shared by
// the eventual mmap and pre-resume fork enrollment paths.
mod coordination;

// Keeps the pre-verity mmap/mprotect hot path free of registry scans. The fork
// participant republishes this derived flag while adopting Mapping records.
static HAS_VERITY_MAPPINGS: AtomicUsize = AtomicUsize::new(0);
static FAULT_INDEX: AtomicUsize = AtomicUsize::new(0);
static FAULT_EPOCH: AtomicUsize = AtomicUsize::new(0);
static FAULT_READERS: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];

#[derive(Clone, Copy)]
struct FaultRange {
    start: usize,
    end: usize,
    protection: c_int,
}

/// This derived immutable index owns only integer ranges, never pointers into
/// Mapping or Clean. Native fault callbacks can therefore classify faults
/// without allocation or taking a mutex already held by the faulting thread.
/// Writers (already serialized by the fork mapping transaction) retire an old
/// index only after every bounded, nonblocking reader has finished with it.
pub(super) fn publish_fault_index(registry: &HashMap<usize, Mapping>) -> Result<(), i32> {
    let mut ranges = Vec::<FaultRange>::new();
    let page = memory_geometry().0;
    for (&start, mapping) in registry {
        let Some(info) = &mapping.verity else {
            continue;
        };
        let end = start.checked_add(mapping.length).ok_or(EINVAL)?;
        let backed_end = info
            .origin
            .checked_add(info.clean.length())
            .ok_or(EINVAL)?
            .min(end);
        let mut append = |from, to| -> Result<(), i32> {
            if let Some(previous) = ranges.last_mut()
                && previous.end == from
                && previous.protection == info.protection
            {
                previous.end = to;
            } else {
                ranges.try_reserve(1).map_err(|_| ENOMEM)?;
                ranges.push(FaultRange {
                    start: from,
                    end: to,
                    protection: info.protection,
                });
            }
            Ok(())
        };
        for at in (start..backed_end).step_by(page) {
            if !info.clean.valid(at - info.origin) {
                append(at, (at + page).min(end))?;
            }
        }
        if backed_end < end {
            append(backed_end.max(start), end)?;
        }
    }
    ranges.sort_unstable_by_key(|entry| entry.start);
    let next = if ranges.is_empty() {
        0
    } else {
        // The vector's allocation is fallible above; this fixed-size owner is
        // deliberately tiny, and belongs to this DLL's derived state only.
        Box::into_raw(Box::new(ranges)) as usize
    };
    let old_epoch = FAULT_EPOCH.load(Ordering::SeqCst);
    let previous = FAULT_INDEX.swap(next, Ordering::SeqCst);
    FAULT_EPOCH.fetch_add(1, Ordering::SeqCst);
    // A reader which validated the old epoch immediately before the swap may
    // see either pointer, so drain that epoch even if the old index was null.
    // New readers use the other epoch and cannot starve retirement indefinitely.
    while FAULT_READERS[old_epoch & 1].load(Ordering::SeqCst) != 0 {
        std::thread::yield_now();
    }
    if previous != 0 {
        unsafe {
            drop(Box::from_raw(previous as *mut Vec<FaultRange>));
        }
    }
    Ok(())
}

pub(super) fn refresh_fault_index() -> Result<(), i32> {
    if !present() {
        return Ok(());
    }
    let registry = mappings().lock().map_err(|_| EIO)?;
    publish_fault_index(&registry)
}

/// The index is derived DLL-local storage, not a fork-transferable object.
/// Adopt only the serialized Mapping/Clean state. Never dereference a parent's
/// index address or wait for a parent's interrupted fault readers in the child.
pub(super) fn reset_child_fault_index() {
    FAULT_INDEX.store(0, Ordering::SeqCst);
    FAULT_EPOCH.store(0, Ordering::SeqCst);
    for readers in &FAULT_READERS {
        readers.store(0, Ordering::SeqCst);
    }
    HAS_VERITY_MAPPINGS.store(0, Ordering::Release);
}

pub(super) fn present() -> bool {
    HAS_VERITY_MAPPINGS.load(Ordering::Acquire) != 0
}
pub(super) fn published() {
    HAS_VERITY_MAPPINGS.store(1, Ordering::Release);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NativeId {
    volume: u64,
    id: [u8; 16],
}

impl NativeId {
    pub(super) fn futex_key(self, offset: u64) -> crate::futex::Key {
        crate::futex::Key::file(self.volume, self.id, offset)
    }
}

pub(super) fn native_id(handle: *mut c_void) -> Result<NativeId, i32> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };
    let mut info: FILE_ID_INFO = unsafe { core::mem::zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&raw mut info).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(last_errno());
    }
    Ok(NativeId {
        volume: info.VolumeSerialNumber,
        id: info.FileId.Identifier,
    })
}

#[repr(C)]
struct CleanHeader {
    references: AtomicUsize,
    length: usize,
    pages: usize,
}

pub(super) struct Clean(*mut CleanHeader);
unsafe impl Send for Clean {}
unsafe impl Sync for Clean {}

impl Clean {
    fn layout(length: usize, pages: usize) -> Result<Layout, i32> {
        Layout::from_size_align(
            size_of::<CleanHeader>()
                .checked_add(pages)
                .and_then(|v| v.checked_add(length))
                .ok_or(ENOMEM)?,
            align_of::<CleanHeader>(),
        )
        .map_err(|_| ENOMEM)
    }

    fn new(handle: *mut c_void, offset: u64, data_length: usize) -> Result<Self, i32> {
        let (page, _) = memory_geometry();
        let length = page_rounded_length(data_length)?;
        let pages = length / page;
        let layout = Self::layout(length, pages)?;
        let raw =
            unsafe { kinakaze_alloc::ManagedAllocator.alloc_zeroed(layout) }.cast::<CleanHeader>();
        if raw.is_null() {
            return Err(ENOMEM);
        }
        unsafe {
            raw.write(CleanHeader {
                references: AtomicUsize::new(1),
                length,
                pages,
            });
        }
        let result = Self(raw);
        for index in 0..pages {
            let at = index * page;
            let count = page.min(data_length - at);
            let buffer = unsafe { core::slice::from_raw_parts_mut(result.data().add(at), count) };
            match fs::verity::verified_read(
                handle,
                offset.checked_add(at as u64).ok_or(EOVERFLOW)?,
                buffer,
            ) {
                Ok(Some(read)) if read == count => {}
                Err(EIO) | Ok(Some(_)) => unsafe {
                    *result.status().add(index) = 1;
                },
                Err(error) => return Err(error),
                _ => return Err(EIO),
            }
        }
        Ok(result)
    }

    fn length(&self) -> usize {
        unsafe { (*self.0).length }
    }
    fn status(&self) -> *mut u8 {
        unsafe { self.0.add(1).cast() }
    }
    fn data(&self) -> *mut u8 {
        unsafe { self.status().add((*self.0).pages) }
    }
    fn valid(&self, offset: usize) -> bool {
        offset < self.length() && unsafe { *self.status().add(offset / memory_geometry().0) == 0 }
    }
    unsafe fn adopt(raw: usize) -> Result<Self, i32> {
        let end = kinakaze_alloc::ARENA_BASE + kinakaze_alloc::ARENA_SIZE;
        if raw < kinakaze_alloc::ARENA_BASE
            || raw % align_of::<CleanHeader>() != 0
            || raw
                .checked_add(size_of::<CleanHeader>())
                .is_none_or(|v| v > end)
        {
            return Err(EINVAL);
        }
        let header = unsafe { &*(raw as *const CleanHeader) };
        if header.length % memory_geometry().0 != 0
            || header.pages != header.length / memory_geometry().0
            || header.references.load(Ordering::Acquire) == 0
            || raw
                .checked_add(Self::layout(header.length, header.pages)?.size())
                .is_none_or(|v| v > end)
        {
            return Err(EINVAL);
        }
        Ok(Self(raw as *mut CleanHeader))
    }
}

impl Clone for Clean {
    fn clone(&self) -> Self {
        unsafe {
            (*self.0).references.fetch_add(1, Ordering::Relaxed);
        }
        Self(self.0)
    }
}
impl Drop for Clean {
    fn drop(&mut self) {
        if unsafe { (*self.0).references.fetch_sub(1, Ordering::AcqRel) } == 1 {
            let layout = unsafe {
                Self::layout((*self.0).length, (*self.0).pages)
                    .expect("validated clean snapshot layout")
            };
            unsafe {
                kinakaze_alloc::ManagedAllocator.dealloc(self.0.cast(), layout);
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct Info {
    clean: Clean,
    origin: usize,
    protection: c_int,
    shared: bool,
}

pub(super) fn encode(mapping: &Mapping, output: &mut [u8]) {
    output.fill(0);
    if let Some(info) = &mapping.verity {
        output[0..8].copy_from_slice(&(info.clean.0 as usize as u64).to_le_bytes());
        output[8..16].copy_from_slice(&(info.origin as u64).to_le_bytes());
        output[16..20].copy_from_slice(&info.protection.to_le_bytes());
        output[20..24].copy_from_slice(&u32::from(info.shared).to_le_bytes());
    }
    if let Some(id) = mapping.native_inode {
        output[24..28].copy_from_slice(&1u32.to_le_bytes());
        output[32..40].copy_from_slice(&id.volume.to_le_bytes());
        output[40..56].copy_from_slice(&id.id);
    }
}

pub(super) fn decode(input: &[u8]) -> Result<(Option<Info>, Option<NativeId>), i32> {
    let raw = u64::from_le_bytes(input[0..8].try_into().unwrap()) as usize;
    let info = if raw == 0 {
        None
    } else {
        let origin = u64::from_le_bytes(input[8..16].try_into().unwrap()) as usize;
        let protection = i32::from_le_bytes(input[16..20].try_into().unwrap());
        let shared = match u32::from_le_bytes(input[20..24].try_into().unwrap()) {
            0 => false,
            1 => true,
            _ => return Err(EINVAL),
        };
        if origin == 0
            || protection & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0
            || (shared && protection & PROT_WRITE != 0)
        {
            return Err(EINVAL);
        }
        published();
        Some(Info {
            clean: unsafe { Clean::adopt(raw)? },
            origin,
            protection,
            shared,
        })
    };
    let native = match u32::from_le_bytes(input[24..28].try_into().unwrap()) {
        0 => None,
        1 => Some(NativeId {
            volume: u64::from_le_bytes(input[32..40].try_into().unwrap()),
            id: input[40..56].try_into().unwrap(),
        }),
        _ => return Err(EINVAL),
    };
    Ok((info, native))
}

pub(super) fn snapshot(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    shared: bool,
    fixed: bool,
    handle: *mut c_void,
    offset: u64,
    size: u64,
) -> Result<*mut c_void, i32> {
    if protection & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
        return Err(EINVAL);
    }
    if shared && protection & PROT_WRITE != 0 {
        return Err(kinakaze_vfs::EACCES);
    }
    let length = page_rounded_length(length)?;
    let data_length =
        usize::try_from(size.saturating_sub(offset).min(length as u64)).map_err(|_| EOVERFLOW)?;
    let clean = Clean::new(handle, offset, data_length)?;
    // Verify before altering a MAP_FIXED destination; failed I/O never leaves
    // an existing neighbouring mapping half-replaced.
    let mapped = if fixed {
        map_anonymous(address, length, PROT_READ | PROT_WRITE, false, true)?
    } else {
        let reserved = map_anonymous(address, length, PROT_NONE, false, false)?;
        if clean.length() != 0 {
            match replace_placeholder_private(reserved, clean.length(), PAGE_READWRITE) {
                Ok(Some(_)) => {}
                result => {
                    let _ = unmap(reserved, length);
                    return Err(result.err().unwrap_or(EIO));
                }
            }
        }
        reserved
    };
    if clean.length() != 0 {
        unsafe {
            ptr::copy_nonoverlapping(clean.data(), mapped.cast::<u8>(), clean.length());
        }
    }
    let info = Info {
        clean,
        origin: mapped as usize,
        protection,
        shared,
    };
    published();
    split_at(info.origin)?;
    split_at(info.origin.checked_add(length).ok_or(EINVAL)?)?;
    let mut registry = mappings().lock().map_err(|_| EIO)?;
    let end = info.origin.checked_add(length).ok_or(EINVAL)?;
    for (&start, mapping) in registry.iter_mut() {
        if start >= info.origin && start < end {
            mapping.verity = Some(info.clone());
            mapping.native_inode = None;
            mapping.file = None;
        }
    }
    drop(registry);
    if let Err(error) = protect(info.origin, end, protection) {
        let _ = unmap(mapped, length);
        return Err(error);
    }
    Ok(mapped)
}

/// Returns runs clipped to VMA boundaries. Ordinary gaps are processed through
/// the original implementation; verified EOF/corruption pages are never made
/// accessible by mprotect, even when the requested Linux protection permits it.
pub(super) fn ranges(start: usize, end: usize) -> Result<Vec<(usize, usize, Option<Info>)>, i32> {
    let registry = mappings().lock().map_err(|_| EIO)?;
    let mut found = registry
        .iter()
        .filter_map(|(&at, mapping)| {
            let to = at.saturating_add(mapping.length).min(end);
            (at < end && to > start).then(|| (at.max(start), to, mapping.verity.clone()))
        })
        .collect::<Vec<_>>();
    found.sort_unstable_by_key(|run| run.0);
    Ok(found)
}

pub(super) fn protect(start: usize, end: usize, protection: c_int) -> Result<(), i32> {
    split_at(start)?;
    split_at(end)?;
    let runs = ranges(start, end)?;
    if runs.iter().any(|(_, _, info)| {
        info.as_ref()
            .is_some_and(|v| v.shared && protection & PROT_WRITE != 0)
    }) {
        return Err(kinakaze_vfs::EACCES);
    }
    let win_protect = page_protection(protection, false)?;
    let page = memory_geometry().0;
    for (from, to, info) in runs {
        let Some(info) = info else {
            continue;
        };
        for at in (from..to).step_by(page) {
            if info.clean.valid(at - info.origin) {
                let mut ignored = 0;
                if unsafe { VirtualProtect(at as *mut c_void, page, win_protect, &mut ignored) }
                    == 0
                {
                    return Err(last_errno());
                }
            } else {
                let mut mbi: MemoryBasicInformation = unsafe { core::mem::zeroed() };
                if unsafe {
                    VirtualQuery(
                        at as *const c_void,
                        &mut mbi,
                        size_of::<MemoryBasicInformation>(),
                    )
                } == 0
                {
                    return Err(last_errno());
                }
                if mbi.state == MEM_COMMIT_STATE {
                    let mut ignored = 0;
                    if unsafe {
                        VirtualProtect(at as *mut c_void, page, PAGE_NOACCESS, &mut ignored)
                    } == 0
                    {
                        return Err(last_errno());
                    }
                }
            }
        }
    }
    // Fragment metadata must track partial mprotect just like the native pages.
    let mut registry = mappings().lock().map_err(|_| EIO)?;
    let affected = registry
        .iter()
        .filter_map(|(&at, mapping)| {
            (mapping.verity.is_some() && at < end && start < at.saturating_add(mapping.length))
                .then(|| (at, mapping.clone()))
        })
        .collect::<Vec<_>>();
    for (at, mapping) in affected {
        let stop = at + mapping.length;
        let from = start.max(at);
        let to = end.min(stop);
        if from != at || to != stop {
            continue;
        }
        if let Some(info) = registry.get_mut(&at).and_then(|m| m.verity.as_mut()) {
            info.protection = protection;
        }
        if let Some(info) = registry.get(&at).and_then(|m| m.file_origin.as_ref()) {
            for address in (from..to).step_by(page) {
                info.owner.set_protection(address, protection)?;
            }
        }
    }
    Ok(())
}

fn split_at(at: usize) -> Result<(), i32> {
    let mut registry = mappings().lock().map_err(|_| EIO)?;
    let Some((start, mapping)) = registry.iter().find_map(|(&start, mapping)| {
        (start < at && at < start.saturating_add(mapping.length)).then(|| (start, mapping.clone()))
    }) else {
        return Ok(());
    };
    let end = start + mapping.length;
    if mapping.kind == MappingKind::PlaceholderView {
        let prepared = prepare_placeholder_locked(&mut registry, at, end - at)?.ok_or(EINVAL)?;
        if let Some(previous) = prepared.previous {
            restore_previous_locked(&mut registry, previous)?;
        }
    } else if mapping.kind == MappingKind::Placeholder {
        if !carve_placeholder_locked(&mut registry, at, end - at)? {
            return Err(EIO);
        }
    } else if matches!(
        mapping.kind,
        MappingKind::Reserved | MappingKind::PlaceholderPrivate
    ) {
        registry.remove(&start);
        unregister_fork_fragment(start, &mapping);
        insert_mapping_fragment(
            &mut registry,
            start,
            Mapping {
                length: at - start,
                ..mapping.clone()
            },
        );
        insert_mapping_fragment(
            &mut registry,
            at,
            Mapping {
                length: end - at,
                ..mapping
            },
        );
    } else {
        return Err(EINVAL);
    }
    Ok(())
}

pub(super) fn mprotect(address: *mut c_void, length: usize, protection: c_int) -> Result<(), i32> {
    let start = address as usize;
    if start % memory_geometry().0 != 0 || protection & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
        return Err(EINVAL);
    }
    let end = start
        .checked_add(page_rounded_length(length)?)
        .ok_or(EINVAL)?;
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;
    let runs = ranges(start, end)?;
    if runs.iter().any(|(_, _, info)| {
        info.as_ref()
            .is_some_and(|v| v.shared && protection & PROT_WRITE != 0)
    }) {
        return Err(kinakaze_vfs::EACCES);
    }
    let mut cursor = start;
    for (from, to, info) in runs {
        if from > cursor {
            return Err(ENOMEM);
        }
        if info.is_some() {
            protect(from, to, protection)?;
        } else if unsafe { mprotect_native(from as *mut c_void, to - from, protection) } != 0 {
            return Err(crate::kinakaze_errno());
        }
        cursor = to;
    }
    if cursor != end {
        return Err(ENOMEM);
    }
    Ok(())
}

pub(super) fn discard(start: usize, end: usize) -> Result<(), i32> {
    for (from, to, info) in ranges(start, end)? {
        let Some(info) = info else {
            continue;
        };
        if info.shared {
            continue;
        }
        for at in (from..to).step_by(memory_geometry().0) {
            let offset = at - info.origin;
            if !info.clean.valid(offset) {
                continue;
            }
            let page = memory_geometry().0;
            let mut previous = 0;
            if unsafe { VirtualProtect(at as *mut c_void, page, PAGE_READWRITE, &mut previous) }
                == 0
            {
                return Err(last_errno());
            }
            unsafe {
                ptr::copy_nonoverlapping(info.clean.data().add(offset), at as *mut u8, page);
            }
            let mut ignored = 0;
            if unsafe { VirtualProtect(at as *mut c_void, page, previous, &mut ignored) } == 0 {
                return Err(last_errno());
            }
        }
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_mapping_fault_signal(address: usize, access: usize) -> c_int {
    let epoch = loop {
        let epoch = FAULT_EPOCH.load(Ordering::SeqCst);
        FAULT_READERS[epoch & 1].fetch_add(1, Ordering::SeqCst);
        if FAULT_EPOCH.load(Ordering::SeqCst) == epoch {
            break epoch;
        }
        FAULT_READERS[epoch & 1].fetch_sub(1, Ordering::SeqCst);
    };
    let index = FAULT_INDEX.load(Ordering::SeqCst);
    let result = if index == 0 {
        0
    } else {
        let ranges = unsafe { &*(index as *const Vec<FaultRange>) };
        let at = ranges.partition_point(|range| range.start <= address);
        if at == 0 {
            0
        } else {
            let range = ranges[at - 1];
            if address >= range.end
                || range.protection == PROT_NONE
                || (access == 1 && range.protection & PROT_WRITE == 0)
                || (access == 8 && range.protection & PROT_EXEC == 0)
            {
                0
            } else {
                7
            }
        }
    };
    FAULT_READERS[epoch & 1].fetch_sub(1, Ordering::SeqCst);
    result
}

pub(crate) fn reject_unverified_mappings(fd: c_int) -> Result<(), i32> {
    let entry = kinakaze_vfs::get(fd)?;
    let id = native_id(entry.raw as *mut c_void)?;
    let registry = mappings().lock().map_err(|_| EIO)?;
    if registry
        .values()
        .any(|mapping| mapping.native_inode == Some(id))
    {
        return Err(kinakaze_vfs::EBUSY);
    }
    Ok(())
}
