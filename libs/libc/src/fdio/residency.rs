//! Linux VMA validation and actual native working-set residency.
use super::*;
use windows_sys::Win32::System::ProcessStatus::{
    PSAPI_WORKING_SET_EX_INFORMATION, QueryWorkingSetEx,
};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtWriteVirtualMemory(
        process: *mut c_void,
        destination: *mut c_void,
        source: *const c_void,
        size: usize,
        copied: *mut usize,
    ) -> i32;
}
pub(super) fn validate(start: usize, end: usize) -> Result<(), i32> {
    let mut cursor = start;
    while cursor < end {
        let mut info: MemoryBasicInformation = unsafe { core::mem::zeroed() };
        if unsafe {
            VirtualQuery(
                cursor as *const _,
                &mut info,
                size_of::<MemoryBasicInformation>(),
            )
        } == 0
        {
            return Err(ENOMEM);
        }
        let limit = (info.base_address as usize)
            .checked_add(info.region_size)
            .ok_or(ENOMEM)?
            .min(end);
        if limit <= cursor {
            return Err(ENOMEM);
        }
        match info.state {
            MEM_COMMIT_STATE => {}
            // Host allocation envelopes also cover Linux holes. Only an actual
            // registered PROT_NONE/uncommitted VMA counts as mapped here.
            MEM_RESERVE_STATE => validate_madvise_range(cursor, limit, false)?,
            _ => return Err(ENOMEM),
        }
        cursor = limit;
    }
    Ok(())
}
pub(crate) fn mincore(start: usize, length: usize, output: usize) -> Result<(), i32> {
    let (page, _) = memory_geometry();
    if start % page != 0 {
        return Err(EINVAL);
    }
    let length = page_rounded_length(length)?;
    let end = start.checked_add(length).ok_or(ENOMEM)?;
    let _tmpfs = kinakaze_vfs::tmpfs::mapping::transaction()?;
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;
    validate(start, end)?;
    let count = length / page;
    if count == 0 {
        return Ok(());
    }
    if output == 0 || output.checked_add(count).is_none() {
        return Err(EFAULT);
    }
    let mut cursor = 0;
    while cursor < count {
        let batch = (count - cursor).min(256);
        let mut pages: [PSAPI_WORKING_SET_EX_INFORMATION; 256] = unsafe { core::mem::zeroed() };
        for (index, entry) in pages[..batch].iter_mut().enumerate() {
            entry.VirtualAddress = (start + (cursor + index) * page) as *mut c_void;
        }
        if unsafe {
            QueryWorkingSetEx(
                GetCurrentProcess(),
                pages.as_mut_ptr().cast(),
                (batch * size_of::<PSAPI_WORKING_SET_EX_INFORMATION>()) as u32,
            )
        } == 0
        {
            return Err(last_errno());
        }
        let mut result = [0u8; 256];
        for (index, entry) in pages[..batch].iter().enumerate() {
            result[index] = (unsafe { entry.VirtualAttributes.Flags } & 1) as u8;
        }
        let mut copied = 0;
        let status = unsafe {
            NtWriteVirtualMemory(
                GetCurrentProcess(),
                (output + cursor) as *mut _,
                result.as_ptr().cast(),
                batch,
                &mut copied,
            )
        };
        if status < 0 || copied != batch {
            return Err(EFAULT);
        }
        cursor += batch;
    }
    Ok(())
}
