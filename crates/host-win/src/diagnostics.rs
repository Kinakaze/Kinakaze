//! On-demand native observations, separate from process lifecycle ownership.
use std::{io, mem::size_of, os::windows::io::AsRawHandle};
use windows_sys::Win32::{
    Foundation::FILETIME,
    System::{
        ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        },
        Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
    },
};

#[derive(Debug)]
pub struct ProcessMetrics {
    pub working_set_bytes: usize,
    pub private_bytes: usize,
    pub cpu_time_ms: u64,
}

fn ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// Reject a reused PID instead of attributing another process's resources.
pub fn process_metrics(pid: u32, birth: u64) -> io::Result<ProcessMetrics> {
    // SAFETY: Scalar arguments; no inheritance. This handle cannot mutate the process.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
    // SAFETY: OpenProcess transfers ownership on success.
    let handle = unsafe { crate::owned(raw)? };
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: The live handle permits queries; all four outputs are writable.
    if unsafe {
        GetProcessTimes(
            handle.as_raw_handle(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if ticks(creation) != birth {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "native process identity changed",
        ));
    }
    let mut memory = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: The EX structure extends COUNTERS and cb describes the entire writable buffer.
    if unsafe {
        GetProcessMemoryInfo(
            handle.as_raw_handle(),
            (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            memory.cb,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(ProcessMetrics {
        working_set_bytes: memory.WorkingSetSize,
        private_bytes: memory.PrivateUsage,
        cpu_time_ms: ticks(kernel).saturating_add(ticks(user)) / 10_000,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn samples_current_identity_and_rejects_stale_birth() {
        let handle = crate::ProcessHandle::open(std::process::id()).unwrap();
        let metrics = super::process_metrics(handle.pid(), handle.birth()).unwrap();
        assert!(metrics.working_set_bytes > 0);
        assert!(metrics.private_bytes > 0);
        assert_eq!(
            super::process_metrics(handle.pid(), handle.birth() ^ 1)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }
}
