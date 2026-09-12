use std::io;
use std::io::IsTerminal;
use std::mem::{size_of, zeroed};
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::ptr::null;
use windows_sys::Win32::Foundation::{FILETIME, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, WaitForSingleObject,
};

/// Redirected streams need no console host. Preserve the existing console path
/// whenever a standard handle is a terminal; inherited console handles require
/// attachment and must not be silently broken by DETACHED_PROCESS.
pub fn background_creation_flags() -> u32 {
    if io::stdin().is_terminal() || io::stdout().is_terminal() || io::stderr().is_terminal() {
        0x0800_0000 // CREATE_NO_WINDOW
    } else {
        0x0000_0008 // DETACHED_PROCESS: no hidden conhost for pipe/file streams
    }
}

/// The priority class CreateProcess would choose without an explicit override.
pub fn inherited_process_priority() -> io::Result<u32> {
    use windows_sys::Win32::System::Threading::{
        BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, GetPriorityClass, IDLE_PRIORITY_CLASS,
        NORMAL_PRIORITY_CLASS,
    };
    match unsafe { GetPriorityClass(GetCurrentProcess()) } {
        0 => Err(io::Error::last_os_error()),
        priority @ (IDLE_PRIORITY_CLASS | BELOW_NORMAL_PRIORITY_CLASS) => Ok(priority),
        _ => Ok(NORMAL_PRIORITY_CLASS),
    }
}

pub fn set_current_process_priority(priority: u32) -> io::Result<()> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass};
    if unsafe { SetPriorityClass(GetCurrentProcess(), priority) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// A pinned kernel process object and its immutable creation identity.
/// Holding this handle prevents PID reuse from changing which process is waited.
pub struct ProcessHandle {
    handle: OwnedHandle,
    pid: u32,
    birth: u64,
}

impl ProcessHandle {
    pub fn open(pid: u32) -> io::Result<Self> {
        // Assignment to a kill-on-close Job requires SET_QUOTA and TERMINATE.
        // SAFETY: No pointers; false explicitly prohibits handle inheritance.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION
                    | PROCESS_SYNCHRONIZE
                    | PROCESS_SET_QUOTA
                    | PROCESS_TERMINATE,
                0,
                pid,
            )
        };
        // SAFETY: OpenProcess transfers ownership on success.
        let handle = unsafe { crate::owned(raw)? };
        let mut creation = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = creation;
        let mut kernel = creation;
        let mut user = creation;
        // SAFETY: This process handle permits queries and all outputs are writable.
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
        let birth = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
        Ok(Self { handle, pid, birth })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn birth(&self) -> u64 {
        self.birth
    }

    /// Native exit status; callers must first observe this pinned process exit.
    pub fn exit_code(&self) -> io::Result<u32> {
        let mut status = 0;
        // SAFETY: owned process handle grants query rights; output is writable.
        if unsafe {
            windows_sys::Win32::System::Threading::GetExitCodeProcess(
                self.handle.as_raw_handle(),
                &mut status,
            )
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(status)
        }
    }

    pub fn wait(&self) -> io::Result<()> {
        self.wait_millis(INFINITE).map(|_| ())
    }

    /// Nonblocking observation of the pinned kernel process object.
    pub fn has_exited(&self) -> io::Result<bool> {
        self.wait_millis(0)
    }

    fn wait_millis(&self, milliseconds: u32) -> io::Result<bool> {
        // SAFETY: The process handle grants SYNCHRONIZE and remains alive.
        match unsafe { WaitForSingleObject(self.handle.as_raw_handle(), milliseconds) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            WAIT_FAILED => Err(io::Error::last_os_error()),
            result => Err(io::Error::other(format!(
                "unexpected process wait result {result:#x}"
            ))),
        }
    }
}

/// Unnamed, non-inheritable job; closing its final handle terminates members.
pub struct Job {
    handle: OwnedHandle,
}

impl Job {
    pub fn new_kill_on_close() -> io::Result<Self> {
        // SAFETY: Null security attributes/name produce a private non-inheritable handle.
        let handle = unsafe { crate::owned(CreateJobObjectW(null(), null()))? };
        // SAFETY: All-zero is a valid empty JOBOBJECT_EXTENDED_LIMIT_INFORMATION.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: Limits is a complete properly aligned structure with its exact size.
        if unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle })
    }

    pub fn assign(&self, process: &ProcessHandle) -> io::Result<()> {
        let mut already_assigned = 0;
        // SAFETY: Both handles remain live; query this exact job, since a child
        // may have inherited it when spawned by an already assigned worker.
        if unsafe {
            IsProcessInJob(
                process.handle.as_raw_handle(),
                self.handle.as_raw_handle(),
                &mut already_assigned,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if already_assigned != 0 {
            return Ok(());
        }
        // SAFETY: Both owned handles remain alive and have the required rights.
        if unsafe {
            AssignProcessToJobObject(self.handle.as_raw_handle(), process.handle.as_raw_handle())
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}
