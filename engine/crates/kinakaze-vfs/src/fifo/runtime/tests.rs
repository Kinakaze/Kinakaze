use super::*;
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::{STILL_ACTIVE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    GetExitCodeProcess, PROCESS_INFORMATION, ResumeThread, STARTUPINFOW, TerminateProcess,
    WaitForSingleObject,
};

struct Fixture {
    root: PathBuf,
    marker: std::fs::File,
}
impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kinakaze-fifo-runtime-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("fifo");
        let marker = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        crate::fs::set_mode_host_path(&path, crate::fs::S_IFIFO | 0o600).unwrap();
        Self { root, marker }
    }
    fn open(&self, flags: i32) -> Descriptor {
        Descriptor(Some(
            open_shared_marker(self.marker.as_raw_handle(), flags).unwrap(),
        ))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

struct Descriptor(Option<i32>);
impl Descriptor {
    fn fd(&self) -> i32 {
        self.0.unwrap()
    }
    fn entry(&self) -> FdEntry {
        crate::get(self.fd()).unwrap()
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        if let Some(fd) = self.0.take() {
            crate::close(fd).unwrap();
        }
    }
}

#[test]
fn fionbio_preserves_append_in_the_shared_fifo_description() {
    let fixture = Fixture::new();
    let endpoint = fixture.open(O_RDWR);
    // F_SETFL updates the channel, not the original descriptor-table flags.
    crate::set_status_flags(endpoint.fd(), true, false).unwrap();
    crate::set_nonblocking(endpoint.fd(), true).unwrap();
    assert_eq!(
        status_flags(endpoint.fd(), endpoint.entry()).unwrap(),
        O_RDWR | O_APPEND | O_NONBLOCK
    );
    assert_eq!(
        read(endpoint.fd(), endpoint.entry(), &mut [0; 1]),
        Err(crate::EAGAIN)
    );
    crate::set_nonblocking(endpoint.fd(), false).unwrap();
    assert_eq!(
        status_flags(endpoint.fd(), endpoint.entry()).unwrap(),
        O_RDWR | O_APPEND
    );
}

#[test]
fn operation_pin_survives_close_and_fd_reuse_cannot_select_a_new_fifo() {
    let first = Fixture::new();
    let second = Fixture::new();
    let original = first.open(O_RDWR | O_NONBLOCK);
    let fd = original.fd();
    let expected = original.entry();
    let pinned = pin(fd, expected).unwrap();
    drop(original);
    let replacement = second.open(O_RDWR | O_NONBLOCK);
    assert_eq!(replacement.fd(), fd);
    assert_eq!(read(fd, expected, &mut [0; 4]), Err(EBADF));
    assert_eq!(pinned.context.write(b"old"), Ok(3));
    let mut bytes = [0; 3];
    assert_eq!(pinned.context.read(&mut bytes), Ok(3));
    assert_eq!(&bytes, b"old");
    assert_eq!(queued_bytes(fd, replacement.entry()), Ok(0));
}

#[test]
fn delayed_close_cannot_leave_an_unenumerated_inheritable_marker_on_fd_reuse() {
    let fixture = Fixture::new();
    let mut original = fixture.open(O_RDWR | O_NONBLOCK);
    let fd = original.fd();
    let pinned = pin(fd, original.entry()).unwrap();
    let detached = crate::table()
        .write()
        .unwrap()
        .slots
        .remove(fd as usize)
        .unwrap();
    original.0 = None;
    let replacement = fixture.open(O_RDWR | O_NONBLOCK);
    assert_eq!(replacement.fd(), fd);
    let mut native_flags = 0;
    assert_ne!(
        unsafe {
            windows_sys::Win32::Foundation::GetHandleInformation(pinned.marker.0, &mut native_flags)
        },
        0
    );
    assert_eq!(native_flags & HANDLE_FLAG_INHERIT, 0);
    close_entry(fd, detached).unwrap();
    assert_eq!(
        write(replacement.fd(), replacement.entry(), b"replacement"),
        Ok(11)
    );
    assert_eq!(pinned.context.read(&mut [0; 11]), Ok(11));
}

#[test]
fn inheritance_exclusion_validates_both_handles_and_rolls_back_actual_flags() {
    use windows_sys::Win32::System::Threading::CreateEventW;
    let first = Owned::checked(unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) }).unwrap();
    let second = Owned::checked(unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) }).unwrap();
    let bits = |handle| {
        let mut bits = 0;
        assert_ne!(unsafe { GetHandleInformation(handle, &mut bits) }, 0);
        bits & HANDLE_FLAG_INHERIT
    };
    set_inheritance_bits(first.0, HANDLE_FLAG_INHERIT).unwrap();
    assert!(
        disable_pair(first.0, ptr::null_mut(), |_, _| {
            panic!("failed query must not mutate either handle")
        })
        .is_err()
    );
    assert_eq!(bits(first.0), HANDLE_FLAG_INHERIT);
    assert_eq!(bits(second.0), 0);
    let mut mutations = 0;
    assert_eq!(
        disable_pair(first.0, second.0, |handle, bits| {
            mutations += 1;
            if mutations == 2 {
                Err(EIO)
            } else {
                set_inheritance_bits(handle, bits)
            }
        }),
        Err(EIO)
    );
    assert_eq!(mutations, 2);
    assert_eq!(bits(first.0), HANDLE_FLAG_INHERIT);
    assert_eq!(bits(second.0), 0);
    disable_pair(first.0, second.0, set_inheritance_bits).unwrap();
    assert_eq!(bits(first.0), 0);
    assert_eq!(bits(second.0), 0);
}

#[test]
fn opath_reopen_is_a_marker_reference_not_a_fifo_endpoint() {
    let fixture = Fixture::new();
    let endpoint = fixture.open(O_RDWR | O_NONBLOCK);
    let pinned = pin(endpoint.fd(), endpoint.entry()).unwrap();
    let fd = reopen_pinned(&pinned, O_PATH | O_CLOEXEC | O_RDWR | O_NONBLOCK | O_APPEND).unwrap();
    let entry = crate::get(fd).unwrap();
    assert_eq!(entry.kind, FdKind::File);
    assert_eq!(
        entry.flags,
        FdFlags::PATH_ONLY.union(FdFlags::CLOSE_ON_EXEC)
    );
    assert_eq!(crate::read(fd, &mut [0; 1]), Err(EBADF));
    assert_eq!(
        crate::fs::fstat(fd).unwrap().st_mode & crate::fs::S_IFMT,
        crate::fs::S_IFIFO
    );
    crate::close(fd).unwrap();
}

#[test]
fn poll_registration_pins_token_and_marker_through_descriptor_close() {
    let fixture = Fixture::new();
    let endpoint = fixture.open(O_RDWR | O_NONBLOCK);
    let (_, waiting) = prepare_wait(endpoint.fd(), endpoint.entry()).unwrap();
    drop(endpoint);
    // A poll syscall's live reference still supplies the reader endpoint.
    let writer = fixture.open(O_WRONLY | O_NONBLOCK);
    assert_eq!(write(writer.fd(), writer.entry(), b"still live"), Ok(10));
    drop(waiting);
    assert!(
        pin(writer.fd(), writer.entry())
            .unwrap()
            .context
            .readiness()
            .unwrap()
            .error
    );
}

struct Child {
    process: Owned,
    thread: Owned,
}
impl Child {
    fn spawn(root: &std::path::Path) -> Self {
        let exe = std::env::current_exe().unwrap();
        let application = super::super::lifecycle::wide(exe.as_os_str());
        let mut command = format!(
            "\"{}\" --exact fifo::runtime::tests::fork_child_driver --ignored --nocapture",
            exe.display()
        )
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
        let mut environment = std::env::vars_os()
            .filter(|(key, _)| {
                !matches!(
                    key.to_string_lossy().to_ascii_uppercase().as_str(),
                    "KINAKAZE_FIFO_FORK_FIXTURE" | "TMP" | "TEMP"
                )
            })
            .map(|(key, value)| format!("{}={}", key.to_string_lossy(), value.to_string_lossy()))
            .collect::<Vec<_>>();
        environment.push(format!("KINAKAZE_FIFO_FORK_FIXTURE={}", root.display()));
        // Change only the spawned child's environment, never this multithreaded
        // test process. A fresh open must still discover the same inode queue.
        environment.push(format!("TMP={}", root.display()));
        environment.push(format!("TEMP={}", root.display()));
        environment.sort_by_key(|value| value.to_uppercase());
        let environment = environment
            .iter()
            .flat_map(|value| value.encode_utf16().chain(Some(0)))
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup.cb = size_of::<STARTUPINFOW>() as u32;
        let mut child: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        assert_ne!(
            unsafe {
                CreateProcessW(
                    application.as_ptr(),
                    command.as_mut_ptr(),
                    ptr::null(),
                    ptr::null(),
                    1,
                    CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
                    environment.as_ptr().cast(),
                    ptr::null(),
                    &startup,
                    &mut child,
                )
            },
            0
        );
        Self {
            process: Owned(child.hProcess),
            thread: Owned(child.hThread),
        }
    }
    fn finish(&self) {
        assert_ne!(unsafe { ResumeThread(self.thread.0) }, u32::MAX);
        assert_eq!(
            unsafe { WaitForSingleObject(self.process.0, 10_000) },
            WAIT_OBJECT_0
        );
        let mut exit = 0;
        assert_ne!(unsafe { GetExitCodeProcess(self.process.0, &mut exit) }, 0);
        assert_eq!(exit, 0);
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        let mut code = 0;
        if unsafe { GetExitCodeProcess(self.process.0, &mut code) } != 0
            && code == STILL_ACTIVE as u32
        {
            unsafe {
                TerminateProcess(self.process.0, 99);
                WaitForSingleObject(self.process.0, 5000);
            }
        }
    }
}

#[test]
fn fork_restore_survives_parent_close_before_child_runs_and_abrupt_child_exit() {
    let fixture = Fixture::new();
    let endpoint = fixture.open(O_RDWR | O_NONBLOCK);
    let reader = fixture.open(O_RDONLY | O_NONBLOCK);
    write(endpoint.fd(), endpoint.entry(), b"parent").unwrap();
    let table = crate::serialize_table().unwrap();
    let extra = serialize_matching(|_| true).unwrap();
    let mut handoff = Vec::new();
    handoff.extend_from_slice(&endpoint.fd().to_le_bytes());
    handoff.extend_from_slice(&(table.len() as u32).to_le_bytes());
    handoff.extend_from_slice(&table);
    handoff.extend_from_slice(&extra);
    std::fs::write(fixture.root.join("handoff.bin"), handoff).unwrap();
    let child = Child::spawn(&fixture.root);
    drop(endpoint);
    let (_, waiting) = prepare_wait(reader.fd(), reader.entry()).unwrap();
    child.finish();
    assert_eq!(
        unsafe { WaitForSingleObject(waiting.handles()[1] as HANDLE, 5000) },
        WAIT_OBJECT_0
    );
    let mut bytes = [0; 8];
    assert_eq!(read(reader.fd(), reader.entry(), &mut bytes), Ok(5));
    assert_eq!(&bytes[..5], b"child");
    assert_eq!(read(reader.fd(), reader.entry(), &mut bytes), Ok(0));
}

#[test]
#[ignore = "launched by the real inherited-HANDLE FIFO restoration test"]
fn fork_child_driver() {
    let root =
        PathBuf::from(std::env::var_os("KINAKAZE_FIFO_FORK_FIXTURE").expect("parent fixture"));
    let payload = std::fs::read(root.join("handoff.bin")).unwrap();
    let fd = i32::from_le_bytes(payload[..4].try_into().unwrap());
    let table_length = u32::from_le_bytes(payload[4..8].try_into().unwrap()) as usize;
    crate::restore_table(&payload[8..8 + table_length]).unwrap();
    assert!(restore_fork_state(&payload[8 + table_length..]));
    let entry = crate::get(fd).unwrap();
    let mut bytes = [0; 6];
    assert_eq!(read(fd, entry, &mut bytes), Ok(6));
    assert_eq!(&bytes, b"parent");
    let pinned = pin(fd, entry).unwrap();
    let independent = Descriptor(Some(
        open_shared_marker(pinned.marker.0, O_RDWR | O_NONBLOCK).unwrap(),
    ));
    assert_eq!(
        write(independent.fd(), independent.entry(), b"shared"),
        Ok(6)
    );
    assert_eq!(read(fd, entry, &mut bytes), Ok(6));
    assert_eq!(&bytes, b"shared");
    set_status_flags(fd, entry, true, true).unwrap();
    assert_eq!(write(fd, entry, b"child"), Ok(5));
    // Deliberately skip Rust/VFS destructors. Kernel HANDLE closure must wake
    // the parent and preserve queued bytes until its live reader consumes them.
    std::process::exit(0);
}
