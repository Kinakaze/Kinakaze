//! Query the native Job's current and peak commit together, without sampling.
//! This excludes section-backed guest mappings; it is not complete Linux memcg
//! accounting, resident memory, or a replacement for section charge ownership.
use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::JobObjects::QueryInformationJobObject;

// Class and ABI used by Microsoft's hcsshim, internal/winapi/jobobject.go.
// https://github.com/microsoft/hcsshim/blob/main/internal/winapi/jobobject.go
// Not exposed by windows-sys 0.61's JobObjects projection.
#[repr(C)]
#[derive(Debug, Default)]
pub(super) struct Commit {
    pub(super) current: u64,
    pub(super) peak: u64,
}

pub(super) fn query(job: HANDLE) -> Result<Commit, i32> {
    // A null handle would query this caller's job, which may be unrelated.
    if job.is_null() || job == INVALID_HANDLE_VALUE {
        return Err(crate::EBADF);
    }
    let mut value = Commit::default();
    let mut returned = 0;
    if unsafe {
        QueryInformationJobObject(
            job,
            28, // JobObjectMemoryUsageInformation
            (&raw mut value).cast(),
            std::mem::size_of::<Commit>() as u32,
            &mut returned,
        )
    } == 0
    {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    if returned != std::mem::size_of::<Commit>() as u32 || value.current > value.peak {
        return Err(crate::EIO);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::ptr;
    use std::sync::mpsc;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    };

    const SIZE: usize = 128 * 1024 * 1024;
    const SLACK: u64 = 8 * 1024 * 1024;

    struct OwnedJob(HANDLE);
    impl Drop for OwnedJob {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn memory_process_helper() {
        if std::env::var_os("KINAKAZE_MEMORY_COMMIT_HELPER").is_none() {
            return;
        }
        let mut input = std::io::stdin().lock().lines();
        assert_eq!(input.next().unwrap().unwrap(), "allocate");
        let p =
            unsafe { VirtualAlloc(ptr::null(), SIZE, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
        assert!(!p.is_null());
        println!("allocated");
        std::io::stdout().flush().unwrap();
        assert_eq!(input.next().unwrap().unwrap(), "release");
        assert_ne!(unsafe { VirtualFree(p, 0, MEM_RELEASE) }, 0);
        println!("released");
        std::io::stdout().flush().unwrap();
        assert_eq!(input.next().unwrap().unwrap(), "exit");
    }

    #[test]
    fn rejects_implicit_or_invalid_job_instead_of_querying_the_host_job() {
        assert_eq!(query(ptr::null_mut()).unwrap_err(), crate::EBADF);
        assert_eq!(query(INVALID_HANDLE_VALUE).unwrap_err(), crate::EBADF);
    }

    #[test]
    fn current_commit_falls_after_release_while_peak_is_retained() {
        let job = OwnedJob(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) });
        assert!(!job.0.is_null());
        assert_eq!(query(job.0).unwrap().current, 0);
        assert_eq!(query(job.0).unwrap().peak, 0);
        let module = module_path!().split_once("::").unwrap().1;
        let mut child = OwnedChild(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    &format!("{module}::memory_process_helper"),
                    "--nocapture",
                ])
                .env("KINAKAZE_MEMORY_COMMIT_HELPER", "1")
                .creation_flags(0x0800_0000)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        assert_ne!(
            unsafe { AssignProcessToJobObject(job.0, child.0.as_raw_handle()) },
            0
        );
        let mut input = child.0.stdin.take().unwrap();
        let output = child.0.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let expect = |want: &str| loop {
            let line = rx
                .recv_timeout(Duration::from_secs(10))
                .expect("helper response timeout")
                .unwrap();
            if line == want {
                break;
            }
        };
        // Exercise the same formatting path used by fixed and mounted cgroups.
        let mut groups = std::collections::HashMap::new();
        groups.insert(
            String::new(),
            super::super::CgroupNode {
                id: 0,
                job: Some(job.0),
                cpu_max_quota: None,
                cpu_max_period: 100_000,
                cpu_weight: 100,
                memory_max: None,
                pids_max: None,
                subtree_control: String::new(),
            },
        );
        let registry = super::super::CgroupRegistry { groups };
        let read = |name: &str| -> u64 {
            String::from_utf8(
                super::super::read_from(&format!("/sys/fs/cgroup/{name}"), &registry).unwrap(),
            )
            .unwrap()
            .trim()
            .parse()
            .unwrap()
        };
        let baseline = read("memory.current");
        input.write_all(b"allocate\n").unwrap();
        expect("allocated");
        let allocated = read("memory.current");
        let peak = read("memory.peak");
        input.write_all(b"release\n").unwrap();
        expect("released");
        let released = read("memory.current");
        let retained_peak = read("memory.peak");
        eprintln!(
            "native commit baseline={baseline} allocated={allocated} released={released} peak={peak} retained_peak={retained_peak}"
        );
        assert!(allocated >= baseline + SIZE as u64 - SLACK);
        assert!(allocated >= released + SIZE as u64 - SLACK);
        assert!(peak >= allocated && retained_peak >= peak);
        input.write_all(b"exit\n").unwrap();
        drop(input);
        reader.join().unwrap();
        assert!(child.0.wait().unwrap().success());
        drop(child);
        // Process teardown can retain small native charges after the signaled
        // exit. Do not overwrite those real charges with a synthetic zero.
        let exited = read("memory.current");
        eprintln!("native commit after exit={exited}");
        assert!(exited <= released + SLACK);
        assert!(read("memory.peak") >= retained_peak);
    }
}
