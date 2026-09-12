//! Linux process_vm_readv/writev, with manager-authorized process identity.
//! https://man7.org/linux/man-pages/man2/process_vm_readv.2.html
use core::ffi::c_void;
use kinakaze_vfs::{EFAULT, EINVAL, ENOMEM, EPERM, ESRCH};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE},
    System::Threading::{
        GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
    },
};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtReadVirtualMemory(
        process: HANDLE,
        source: *const c_void,
        destination: *mut c_void,
        size: usize,
        copied: *mut usize,
    ) -> i32;
    fn NtWriteVirtualMemory(
        process: HANDLE,
        destination: *mut c_void,
        source: *const c_void,
        size: usize,
        copied: *mut usize,
    ) -> i32;
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct IoVec {
    pub base: usize,
    pub length: usize,
}

struct Process(HANDLE, bool);
impl Drop for Process {
    fn drop(&mut self) {
        if self.1 {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

fn local_pid() -> u32 {
    if kinakaze_runtime::authority::get().is_some() {
        kinakaze_runtime::authority::process_id()
    } else {
        std::process::id()
    }
}

fn target(pid: i32, write: bool) -> Result<Process, i32> {
    if pid <= 0 {
        return Err(ESRCH);
    }
    if pid as u32 == local_pid() {
        return Ok(Process(unsafe { GetCurrentProcess() }, false));
    }
    let (native, birth) = kinakaze_runtime::authority::memory_target(pid as u32, write)?;
    let access = PROCESS_QUERY_LIMITED_INFORMATION
        | if write {
            PROCESS_VM_OPERATION | PROCESS_VM_WRITE
        } else {
            PROCESS_VM_READ
        };
    let handle = unsafe { OpenProcess(access, 0, native) };
    if handle.is_null() {
        return Err(EPERM);
    }
    let process = Process(handle, true);
    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut unused = [created; 3];
    let result = unsafe {
        GetProcessTimes(
            handle,
            &mut created,
            &mut unused[0],
            &mut unused[1],
            &mut unused[2],
        )
    };
    let actual = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    if result == 0 || actual != birth {
        return Err(ESRCH);
    }
    Ok(process)
}

fn vectors(pointer: usize, count: usize) -> Result<(Vec<IoVec>, usize), i32> {
    if count > 1024 {
        return Err(EINVAL);
    }
    if count == 0 {
        return Ok((Vec::new(), 0));
    }
    let length = count * core::mem::size_of::<IoVec>();
    if pointer == 0 || pointer.checked_add(length).is_none() {
        return Err(EFAULT);
    }
    let mut vectors = Vec::new();
    vectors.try_reserve_exact(count).map_err(|_| ENOMEM)?;
    vectors.resize(count, IoVec::default());
    let mut copied = 0;
    let status = unsafe {
        NtReadVirtualMemory(
            GetCurrentProcess(),
            pointer as _,
            vectors.as_mut_ptr().cast(),
            length,
            &mut copied,
        )
    };
    if status < 0 || copied != length {
        return Err(EFAULT);
    }
    let mut total = 0usize;
    for vector in &vectors {
        total = total
            .checked_add(vector.length)
            .filter(|total| *total <= isize::MAX as usize)
            .ok_or(EINVAL)?;
    }
    Ok((vectors, total))
}

/// Inputs are guest addresses, read using fault-contained native copies. Raw
/// syscall and libc wrappers share this operation, including partial transfers.
pub fn transfer(
    pid: i32,
    local: usize,
    local_count: usize,
    remote: usize,
    remote_count: usize,
    flags: usize,
    write: bool,
) -> Result<usize, i32> {
    if flags != 0 || local_count > 1024 || remote_count > 1024 {
        return Err(EINVAL);
    }
    let (local, local_length) = vectors(local, local_count)?;
    if local_length == 0 {
        return Ok(0);
    }
    if local.iter().any(|item| {
        item.length != 0 && (item.base == 0 || item.base.checked_add(item.length).is_none())
    }) {
        return Err(EFAULT);
    }
    let (remote, remote_length) = vectors(remote, remote_count)?;
    if remote_length == 0 {
        return Ok(0);
    }
    let process = target(pid, write)?;
    let mut total = 0;
    let (mut li, mut lo, mut ri, mut ro) = (0, 0, 0, 0);
    while li < local.len() && ri < remote.len() {
        if lo == local[li].length {
            li += 1;
            lo = 0;
            continue;
        }
        if ro == remote[ri].length {
            ri += 1;
            ro = 0;
            continue;
        }
        let local_address = local[li].base + lo;
        let Some(remote_address) = remote[ri].base.checked_add(ro) else {
            return if total == 0 { Err(EFAULT) } else { Ok(total) };
        };
        // Faults beyond this page retain the successfully copied prefix. Never
        // use WriteProcessMemory: its debugger path can bypass page protection.
        let length = (local[li].length - lo)
            .min(remote[ri].length - ro)
            .min(4096 - (local_address & 4095))
            .min(4096 - (remote_address & 4095));
        let mut copied = 0;
        let status = unsafe {
            if write {
                NtWriteVirtualMemory(
                    process.0,
                    remote_address as _,
                    local_address as _,
                    length,
                    &mut copied,
                )
            } else {
                NtReadVirtualMemory(
                    process.0,
                    remote_address as _,
                    local_address as _,
                    length,
                    &mut copied,
                )
            }
        };
        if copied > length {
            return if total == 0 { Err(EFAULT) } else { Ok(total) };
        }
        total += copied;
        if status < 0 || copied != length {
            return if total == 0 { Err(EFAULT) } else { Ok(total) };
        }
        lo += copied;
        ro += copied;
    }
    Ok(total)
}

fn posix(result: Result<usize, i32>) -> isize {
    match result {
        Ok(count) => count as isize,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_process_vm_readv(
    pid: i32,
    local: *const IoVec,
    local_count: usize,
    remote: *const IoVec,
    remote_count: usize,
    flags: usize,
) -> isize {
    posix(transfer(
        pid,
        local as usize,
        local_count,
        remote as usize,
        remote_count,
        flags,
        false,
    ))
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_process_vm_writev(
    pid: i32,
    local: *const IoVec,
    local_count: usize,
    remote: *const IoVec,
    remote_count: usize,
    flags: usize,
) -> isize {
    posix(transfer(
        pid,
        local as usize,
        local_count,
        remote as usize,
        remote_count,
        flags,
        true,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scatter_gather_read_and_write() {
        let mut first = [1u8, 2, 3];
        let mut second = [4u8, 5];
        let mut output = [0u8; 5];
        let local = [IoVec {
            base: output.as_mut_ptr() as usize,
            length: 5,
        }];
        let remote = [
            IoVec {
                base: first.as_mut_ptr() as usize,
                length: 3,
            },
            IoVec {
                base: second.as_mut_ptr() as usize,
                length: 2,
            },
        ];
        assert_eq!(
            transfer(
                local_pid() as i32,
                local.as_ptr() as usize,
                1,
                remote.as_ptr() as usize,
                2,
                0,
                false
            ),
            Ok(5)
        );
        assert_eq!(output, [1, 2, 3, 4, 5]);
        output.reverse();
        assert_eq!(
            transfer(
                local_pid() as i32,
                local.as_ptr() as usize,
                1,
                remote.as_ptr() as usize,
                2,
                0,
                true
            ),
            Ok(5)
        );
        assert_eq!(first, [5, 4, 3]);
        assert_eq!(second, [2, 1]);
    }
    #[test]
    fn invalid_remote_retains_prefix_and_validates_vectors() {
        let input = [7u8; 3];
        let mut output = [0u8; 5];
        let local = [IoVec {
            base: output.as_mut_ptr() as usize,
            length: 5,
        }];
        let remote = [
            IoVec {
                base: input.as_ptr() as usize,
                length: 3,
            },
            IoVec { base: 0, length: 2 },
        ];
        assert_eq!(
            transfer(
                local_pid() as i32,
                local.as_ptr() as usize,
                1,
                remote.as_ptr() as usize,
                2,
                0,
                false
            ),
            Ok(3)
        );
        assert_eq!(output, [7, 7, 7, 0, 0]);
        assert_eq!(transfer(1, 0, 0, 0, 0, 1, false), Err(EINVAL));
        assert_eq!(transfer(1, 0, 1025, 0, 0, 0, false), Err(EINVAL));
        assert_eq!(transfer(1, 1, 1, 0, 0, 0, false), Err(EFAULT));
        assert_eq!(transfer(-1, 0, 0, 0, 0, 0, false), Ok(0));
    }
    #[test]
    fn cannot_write_readonly_memory() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READONLY, VirtualAlloc, VirtualFree,
        };
        let page = unsafe {
            VirtualAlloc(
                core::ptr::null(),
                4096,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READONLY,
            )
        };
        assert!(!page.is_null());
        let input = [9u8];
        let local = [IoVec {
            base: input.as_ptr() as usize,
            length: 1,
        }];
        let remote = [IoVec {
            base: page as usize,
            length: 1,
        }];
        assert_eq!(
            transfer(
                local_pid() as i32,
                local.as_ptr() as usize,
                1,
                remote.as_ptr() as usize,
                1,
                0,
                true
            ),
            Err(EFAULT)
        );
        assert_eq!(unsafe { *(page as *const u8) }, 0);
        assert_ne!(unsafe { VirtualFree(page, 0, MEM_RELEASE) }, 0);
    }
}
