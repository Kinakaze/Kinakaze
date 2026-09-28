//! Pagefile-backed tmpfs uses the same VMA splitting and fork transport as other
//! retained sections. No staging file, copied MAP_SHARED data, or private fd is
//! needed; one backing owns the lease for every fragment of this mapping.
use super::*;

pub(super) fn map(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    shared: bool,
    fixed: bool,
    fd: i32,
    offset: u64,
) -> Result<*mut c_void, i32> {
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;
    let length = page_rounded_length(length)?;
    let view = kinakaze_vfs::tmpfs::mapping::prepare(fd, offset, length, shared, protection)?;
    let maximum = page_protection(view.maximum, !shared)?;
    let current = page_protection(protection, !shared)?;
    let capacity = view.capacity;
    let identity = view.identity;
    let runs = view.runs.clone();
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
