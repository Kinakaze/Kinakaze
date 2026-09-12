#![cfg(windows)]

use kinakaze_v2_host_win::{
    ExecutableMemory, Job, Library, MemoryProtection, PipeConnection, PipeListener, ProcessHandle,
    page_size, random_token,
};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn endpoint() -> String {
    format!(
        r"\\.\pipe\kinakaze-v2-test-{}-{}",
        std::process::id(),
        random_token().unwrap()
    )
}

#[test]
fn pipe_roundtrip_peer_and_continuous_exclusive_bind() {
    let name = endpoint();
    let mut listener = PipeListener::bind(&name).unwrap();
    assert!(PipeListener::bind(&name).is_err());
    let client_name = name.clone();
    let client = std::thread::spawn(move || {
        let mut stream = PipeConnection::connect(&client_name).unwrap();
        assert_eq!(stream.peer_pid().unwrap(), std::process::id());
        stream.write_all(b"request").unwrap();
        let mut reply = [0; 5];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"reply");
    });
    let mut stream = listener.accept().unwrap();
    assert_eq!(stream.peer_pid().unwrap(), std::process::id());
    assert!(PipeListener::bind(&name).is_err());
    let mut request = [0; 7];
    stream.read_exact(&mut request).unwrap();
    assert_eq!(&request, b"request");
    stream.write_all(b"reply").unwrap();
    client.join().unwrap();
    assert_eq!(stream.read(&mut [0]).unwrap(), 0);
    drop(stream);
    assert!(PipeListener::bind(&name).is_err());
    drop(listener);
    assert!(PipeListener::bind(&name).is_ok());
}

#[test]
fn dropped_client_does_not_poison_listener() {
    let name = endpoint();
    let mut listener = PipeListener::bind(&name).unwrap();
    drop(PipeConnection::connect(&name).unwrap());
    let client_name = name.clone();
    let client = std::thread::spawn(move || {
        let mut stream = PipeConnection::connect(&client_name).unwrap();
        stream.write_all(b"ok").unwrap();
    });
    let mut stream = listener.accept().unwrap();
    let mut bytes = [0; 2];
    stream.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"ok");
    client.join().unwrap();
}

#[test]
fn invalid_endpoints_are_rejected_before_native_open() {
    for name in [
        r"\\remote\pipe\service",
        r"\\.\pipe\",
        r"\\.\pipe\bad\name",
        "bad\0name",
    ] {
        assert!(PipeListener::bind(name).is_err());
        assert!(PipeConnection::connect(name).is_err());
    }
}

#[test]
fn pinned_process_identity_is_stable() {
    let first = ProcessHandle::open(std::process::id()).unwrap();
    let second = ProcessHandle::open(std::process::id()).unwrap();
    assert_eq!(first.pid(), std::process::id());
    assert_ne!(first.birth(), 0);
    assert_eq!(first.birth(), second.birth());
    assert!(!first.has_exited().unwrap());
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn closing_job_terminates_assigned_process() {
    let name = endpoint();
    let mut listener = PipeListener::bind(&name).unwrap();
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "job_child_wait", "--ignored", "--nocapture"])
            .env("KINAKAZEV2_JOB_CHILD", &name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let process = ProcessHandle::open(child.0.id()).unwrap();
    let job = Job::new_kill_on_close().unwrap();
    job.assign(&process).unwrap();
    job.assign(&process).unwrap();
    let mut connection = listener.accept().unwrap();
    assert_eq!(connection.peer_pid().unwrap(), child.0.id());
    let mut ready = [0; 5];
    connection.read_exact(&mut ready).unwrap();
    assert_eq!(&ready, b"ready");
    assert!(!process.has_exited().unwrap());
    drop(job);
    process.wait().unwrap();
    assert!(process.has_exited().unwrap());
    child.0.wait().unwrap();
    assert_eq!(connection.read(&mut [0]).unwrap(), 0);
}

#[test]
#[ignore = "helper process intentionally waits for parent Job to terminate it"]
fn job_child_wait() {
    let name =
        std::env::var("KINAKAZEV2_JOB_CHILD").expect("test helper requires a parent endpoint");
    let mut connection = PipeConnection::connect(&name).unwrap();
    connection.write_all(b"ready").unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn dll_uses_absolute_path_and_typed_export() {
    assert!(Library::open(Path::new("kernel32.dll")).is_err());
    let library = Library::open(
        &PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("kernel32.dll"),
    )
    .unwrap();
    // SAFETY: kernel32 exports this Windows ABI function with no arguments.
    let get_pid: unsafe extern "system" fn() -> u32 =
        unsafe { std::mem::transmute(library.symbol(c"GetCurrentProcessId").unwrap()) };
    // SAFETY: Signature above matches the export; the library owner remains live.
    assert_eq!(unsafe { get_pid() }, std::process::id());
    // SAFETY: A nonexistent export returns an error without calling any address.
    assert!(unsafe { library.symbol(c"kinakaze_nonexistent_export") }.is_err());
}

#[test]
fn memory_checks_whole_pages_and_executes_after_rx_transition() {
    let page = page_size();
    let region = ExecutableMemory::allocate(page + 1).unwrap();
    assert_eq!(region.len(), 2 * page);
    assert!(region.protect(1, page, MemoryProtection::ReadOnly).is_err());
    assert!(
        region
            .protect(0, page + 1, MemoryProtection::ReadOnly)
            .is_err()
    );
    assert!(
        region
            .protect(2 * page, page, MemoryProtection::ReadOnly)
            .is_err()
    );
    assert!(ExecutableMemory::allocate(usize::MAX).is_err());
    // SAFETY: Allocation starts readable/writable and has at least two pages.
    unsafe {
        assert_eq!(*region.as_ptr().add(page), 0);
        *region.as_ptr().add(page) = 17;
    }
    region
        .protect(page, page, MemoryProtection::ReadOnly)
        .unwrap();
    #[cfg(target_arch = "x86_64")]
    {
        // mov eax, 42; ret; valid for the x86-64 integer return ABI.
        let code = [0xb8u8, 42, 0, 0, 0, 0xc3];
        // SAFETY: The first page remains RW and is large enough for these bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), region.as_ptr(), code.len());
        }
        region
            .protect(0, page, MemoryProtection::ReadExecute)
            .unwrap();
        // SAFETY: The bytes above implement this exact signature in live RX memory.
        let entry: unsafe extern "C" fn() -> u32 = unsafe { std::mem::transmute(region.as_ptr()) };
        // SAFETY: Region remains live and the entry has no data dependencies.
        assert_eq!(unsafe { entry() }, 42);
    }
}
