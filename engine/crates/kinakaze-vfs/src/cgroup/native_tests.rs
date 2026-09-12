//! Native capability probes. Passing these does not enable a Linux controller:
//! hierarchy, accounting and lifecycle semantics must also be implemented.

use std::ffi::c_void;
use std::io::{BufRead, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessorNumber, GetCurrentThread, GetProcessAffinityMask,
    SetThreadAffinityMask,
};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCreatePartition(
        parent: HANDLE,
        partition: *mut HANDLE,
        access: u32,
        attrs: *const c_void,
    ) -> i32;
}

struct Owned(HANDLE);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

#[test]
#[ignore = "manual capability probe; never enables privileges or changes host paging"]
fn native_memory_partition_creation() {
    let mut handle = ptr::null_mut();
    let status = unsafe { NtCreatePartition(ptr::null_mut(), &mut handle, 3, ptr::null()) };
    eprintln!(
        "NtCreatePartition status={:#010x} handle={handle:?}",
        status as u32
    );
    if status >= 0 {
        assert!(!handle.is_null());
        drop(Owned(handle));
    }
}

#[test]
fn constrained_process_helper() {
    let Ok(mode) = std::env::var("KINAKAZE_JOB_PROBE_MODE") else {
        return;
    };
    // The parent installs the constraint before releasing this gate.
    let mut input = String::new();
    std::io::stdin().lock().read_line(&mut input).unwrap();
    assert_eq!(input.trim(), "go");
    if mode == "commit" {
        let first = unsafe {
            VirtualAlloc(
                ptr::null(),
                80 * 1024 * 1024,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        assert!(!first.is_null(), "first commit: {}", unsafe {
            GetLastError()
        });
        let mut counters: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
        counters.cb = std::mem::size_of_val(&counters) as u32;
        assert_ne!(
            unsafe {
                K32GetProcessMemoryInfo(
                    GetCurrentProcess(),
                    (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
                    counters.cb,
                )
            },
            0
        );
        let second = unsafe {
            VirtualAlloc(
                ptr::null(),
                80 * 1024 * 1024,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        let second_error = unsafe { GetLastError() };
        println!(
            "COMMIT private={} working_set={} second_failed={} error={second_error}",
            counters.PrivateUsage,
            counters.WorkingSetSize,
            second.is_null()
        );
        if !second.is_null() {
            unsafe { VirtualFree(second, 0, MEM_RELEASE) };
        }
        unsafe { VirtualFree(first, 0, MEM_RELEASE) };
        assert!(second.is_null(), "job commit ceiling was not enforced");
        assert!(
            counters.WorkingSetSize < 80 * 1024 * 1024,
            "probe unexpectedly resident"
        );
    } else {
        let allowed: usize = std::env::var("KINAKAZE_JOB_PROBE_MASK")
            .unwrap()
            .parse()
            .unwrap();
        let mut process_mask = 0;
        let mut system_mask = 0;
        assert_ne!(
            unsafe {
                GetProcessAffinityMask(GetCurrentProcess(), &mut process_mask, &mut system_mask)
            },
            0
        );
        assert_eq!(process_mask, allowed);
        let excluded = system_mask & !allowed;
        if excluded != 0 {
            let outside = 1usize << excluded.trailing_zeros();
            assert_eq!(
                unsafe { SetThreadAffinityMask(GetCurrentThread(), outside) },
                0,
                "thread escaped job affinity"
            );
        }
        for _ in 0..10_000 {
            let cpu = unsafe { GetCurrentProcessorNumber() };
            assert_ne!(allowed & (1usize << cpu), 0);
        }
        println!("AFFINITY allowed={allowed:#x} samples=10000 escape_rejected=true");
    }
}

fn run_constrained(mode: &str) -> String {
    let job = Owned(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) });
    assert!(!job.0.is_null());
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    let mut mask = 0;
    if mode == "commit" {
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY;
        limits.JobMemoryLimit = 128 * 1024 * 1024;
    } else {
        let mut system = 0;
        assert_ne!(
            unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut mask, &mut system) },
            0
        );
        assert_ne!(mask, 0);
        mask = 1usize << mask.trailing_zeros();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_AFFINITY | JOB_OBJECT_LIMIT_SUBSET_AFFINITY;
        limits.BasicLimitInformation.Affinity = mask;
    }
    assert_ne!(
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        },
        0
    );
    let module = module_path!()
        .split_once("::")
        .map(|(_, module)| format!("{module}::"))
        .unwrap_or_default();
    let helper = format!("{module}constrained_process_helper");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &helper, "--nocapture"])
        .env("KINAKAZE_JOB_PROBE_MODE", mode)
        .env("KINAKAZE_JOB_PROBE_MASK", mask.to_string())
        .creation_flags(0x0800_0000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_ne!(
        unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle()) },
        0,
        "job assignment: {}",
        unsafe { GetLastError() }
    );
    child.stdin.take().unwrap().write_all(b"go\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn job_commit_limit_is_not_a_resident_or_swap_limit() {
    eprintln!("{}", run_constrained("commit"));
}

#[test]
fn job_affinity_enforces_a_hard_cpu_boundary() {
    eprintln!("{}", run_constrained("affinity"));
}
