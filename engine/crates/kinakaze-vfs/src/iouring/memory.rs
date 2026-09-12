//! Fault-contained copies. Native IoRing only sees owned allocations.
use super::{IORING_OP_READV, IORING_OP_WRITEV, IoVec, SubmissionEntry};
use crate::{EFAULT, EINVAL};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn ReadProcessMemory(
        process: HANDLE,
        source: *const core::ffi::c_void,
        destination: *mut core::ffi::c_void,
        length: usize,
        copied: *mut usize,
    ) -> i32;
}
#[derive(Clone, Copy)]
pub(super) struct Segment {
    pub address: usize,
    pub length: usize,
}
fn region_prefix(address: usize, length: usize) -> Result<usize, i32> {
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
    let mut information: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        VirtualQuery(
            address as *const _,
            &mut information,
            std::mem::size_of_val(&information),
        )
    } == 0
    {
        return Err(EFAULT);
    }
    let end = (information.BaseAddress as usize)
        .checked_add(information.RegionSize)
        .ok_or(EFAULT)?;
    Ok(length.min(end.checked_sub(address).ok_or(EFAULT)?))
}
fn attempt(source: usize, destination: usize, length: usize) -> Result<usize, i32> {
    let mut copied = 0;
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            source as *const _,
            destination as *mut _,
            length,
            &mut copied,
        )
    };
    if copied > length {
        return Err(EFAULT);
    }
    if copied != 0 || ok != 0 {
        Ok(copied)
    } else {
        Err(EFAULT)
    }
}
pub(super) fn copy(source: usize, destination: usize, length: usize) -> Result<usize, i32> {
    if length == 0 {
        return Ok(0);
    }
    if source == 0
        || destination == 0
        || source.checked_add(length).is_none()
        || destination.checked_add(length).is_none()
    {
        return Err(EFAULT);
    }
    // Unlike WriteProcessMemory, this does not make readonly destinations
    // writable for debugger patching. Kernel probes cover concurrent unmaps.
    let mut total = 0;
    while total < length {
        let remaining = length - total;
        let from = source + total;
        let to = destination + total;
        // Ordinary valid buffers take exactly one kernel copy. RPM can reject
        // an entire cross-region range without reporting its valid prefix; on
        // that fault only, split at native region boundaries to retain Linux's
        // successfully transferred prefix. This is memory probing, not file I/O.
        let copied = attempt(from, to, remaining).or_else(|error| {
            let prefix = region_prefix(from, remaining)?.min(region_prefix(to, remaining)?);
            if prefix == 0 || prefix == remaining {
                Err(error)
            } else {
                attempt(from, to, prefix)
            }
        });
        match copied {
            Ok(0) | Err(_) => return if total == 0 { Err(EFAULT) } else { Ok(total) },
            Ok(copied) => total += copied,
        }
    }
    Ok(total)
}
pub(super) fn segments(entry: &SubmissionEntry) -> Result<Vec<Segment>, i32> {
    const MAX_RW_COUNT: usize = 0x7fff_f000;
    let vectored = matches!(entry.opcode, IORING_OP_READV | IORING_OP_WRITEV);
    let count = if vectored { entry.length as usize } else { 1 };
    if count > 1024 {
        return Err(EINVAL);
    }
    let mut segments = Vec::new();
    segments
        .try_reserve_exact(count)
        .map_err(|_| crate::ENOMEM)?;
    let mut total = 0usize;
    for index in 0..count {
        let (address, length) = if vectored {
            let offset = index
                .checked_mul(std::mem::size_of::<IoVec>())
                .ok_or(EFAULT)?;
            let address = (entry.address as usize).checked_add(offset).ok_or(EFAULT)?;
            let mut item = IoVec {
                base: std::ptr::null_mut(),
                length: 0,
            };
            if copy(
                address,
                &mut item as *mut IoVec as usize,
                std::mem::size_of::<IoVec>(),
            )? != std::mem::size_of::<IoVec>()
            {
                return Err(EFAULT);
            }
            (item.base as usize, item.length)
        } else {
            (entry.address as usize, entry.length as usize)
        };
        if length > isize::MAX as usize {
            return Err(EINVAL);
        }
        if address.checked_add(length).is_none() {
            return Err(EFAULT);
        }
        let length = length.min(MAX_RW_COUNT - total);
        total += length;
        segments.push(Segment { address, length });
    }
    Ok(segments)
}
pub(super) fn gather(segments: &[Segment], bytes: &mut [u8]) -> Result<usize, i32> {
    let mut offset = 0;
    for segment in segments {
        match copy(
            segment.address,
            bytes.as_mut_ptr() as usize + offset,
            segment.length,
        ) {
            Ok(count) => {
                offset += count;
                if count != segment.length {
                    return Ok(offset);
                }
            }
            Err(error) => return if offset == 0 { Err(error) } else { Ok(offset) },
        }
    }
    Ok(offset)
}
pub(super) fn scatter(bytes: &[u8], segments: &[Segment]) -> Result<usize, i32> {
    let mut offset = 0;
    for segment in segments {
        let length = segment.length.min(bytes.len() - offset);
        match copy(bytes.as_ptr() as usize + offset, segment.address, length) {
            Ok(count) => {
                offset += count;
                if count != length {
                    return Ok(offset);
                }
            }
            Err(error) => return if offset == 0 { Err(error) } else { Ok(offset) },
        }
        if offset == bytes.len() {
            break;
        }
    }
    Ok(offset)
}
