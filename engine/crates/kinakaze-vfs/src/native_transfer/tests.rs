use super::*;
use std::io::{Read, Write};
use std::os::windows::io::{AsHandle, FromRawHandle, IntoRawHandle, OwnedHandle};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, GetProcessHandleCount,
};

fn frame(sections: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut frame = crate::FORK_STATE_MAGIC.to_le_bytes().to_vec();
    frame.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    frame.extend_from_slice(&0u32.to_le_bytes());
    for (tag, bytes) in sections {
        crate::append_fork_section(&mut frame, *tag, bytes).unwrap();
    }
    frame
}

fn event() -> OwnedHandle {
    let raw = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    assert!(!raw.is_null());
    unsafe { OwnedHandle::from_raw_handle(raw) }
}

fn handle_count(process: &impl AsRawHandle) -> u32 {
    let mut count = 0;
    assert_ne!(
        unsafe { GetProcessHandleCount(process.as_raw_handle(), &mut count) },
        0
    );
    count
}

#[test]
fn indexes_are_scoped_deduplicated_and_strictly_validated() {
    assert_eq!(encode(40), 40);
    let captured = capture(|| {
        assert_eq!(encode(0), 0);
        assert_eq!(encode(40), TOKEN | 1);
        assert_eq!(encode(40), TOKEN | 1);
        assert_eq!(encode(48), TOKEN | 2);
        assert_eq!(capture(|| Ok(frame(&[]))), Err(EBUSY));
        Ok(frame(&[(91, b"uninterpreted bytes".to_vec())]))
    })
    .unwrap();
    assert!(!capturing());
    assert_eq!(decode(TOKEN | 1), Err(EIO));
    {
        let _scope = restore_scope(&captured).unwrap();
        assert_eq!(decode(0), Ok(0));
        assert_eq!(decode(TOKEN | 1), Ok(40));
        assert_eq!(decode(TOKEN | 2), Ok(48));
        for invalid in [40, TOKEN, TOKEN | 3, u64::MAX] {
            assert_eq!(decode(invalid), Err(EIO));
        }
        assert!(matches!(restore_scope(&captured), Err(EBUSY)));
        assert!(matches!(restore_scope(&frame(&[])), Err(EBUSY)));
    }
    assert_eq!(decode(40), Ok(40));
    let range = table_range(&captured).unwrap().unwrap();
    for offset in [range.start + 16, range.start + 24] {
        let mut invalid = captured.clone();
        invalid[offset + 16..offset + 24].copy_from_slice(&captured[offset..offset + 8]);
        assert!(
            restore_scope(&invalid).is_err(),
            "duplicate source or destination"
        );
    }
    for length in 0..captured.len() {
        assert!(restore_scope(&captured[..length]).is_err());
    }
    let mut invalid = captured.clone();
    invalid.push(0);
    assert!(restore_scope(&invalid).is_err());
    let duplicate_section = frame(&[
        (SECTION, captured[range.clone()].to_vec()),
        (SECTION, captured[range].to_vec()),
    ]);
    assert!(restore_scope(&duplicate_section).is_err());
}

#[test]
fn capture_and_restore_scopes_unwind_after_errors_or_panics() {
    assert_eq!(capture(|| Err(ENOMEM)), Err(ENOMEM));
    assert!(!capturing());
    let panic = std::panic::catch_unwind(|| {
        capture(|| -> Result<Vec<u8>, i32> {
            encode(40);
            panic!("capture test");
        })
    });
    assert!(panic.is_err());
    assert!(!capturing());
    let captured = capture(|| {
        encode(40);
        Ok(frame(&[]))
    })
    .unwrap();
    let panic = std::panic::catch_unwind(|| {
        let _scope = restore_scope(&captured).unwrap();
        panic!("restore test");
    });
    assert!(panic.is_err());
    assert_eq!(decode(40), Ok(40));
}

#[test]
fn portable_descriptors_preserve_flags_and_keep_socket_recipe_keys() {
    use crate::{FdEntry, FdFlags as F, FdKind};
    let entry = FdEntry {
        raw: 40,
        kind: FdKind::File,
        flags: F::BORROWED.union(F::CLOSE_ON_EXEC).union(F::READ_ACCESS),
        generation: 3,
        description_id: 7,
        offset: 11,
    };
    assert_eq!(descriptor(entry), Ok((40, entry.flags)));
    capture(|| {
        let (raw, flags) = descriptor(entry).unwrap();
        assert_eq!(raw, TOKEN | 1);
        assert!(!flags.contains(F::BORROWED));
        assert!(flags.contains(F::CLOSE_ON_EXEC));
        assert!(flags.contains(F::READ_ACCESS));
        assert_eq!(
            descriptor(FdEntry {
                kind: FdKind::Socket,
                ..entry
            })
            .unwrap()
            .0,
            40
        );
        for kind in [FdKind::Console, FdKind::IoRing] {
            assert_eq!(
                descriptor(FdEntry { kind, ..entry }),
                Err(crate::EOPNOTSUPP)
            );
        }
        Ok(frame(&[]))
    })
    .unwrap();
}

#[test]
fn failed_transfer_closes_partial_duplicates_and_preserves_frame() {
    let first = event();
    let mut captured = capture(|| {
        encode(first.as_raw_handle() as u64);
        encode(TOKEN - 4); // neither a live handle nor a supported pseudo handle
        Ok(frame(&[]))
    })
    .unwrap();
    let original = captured.clone();
    let process = unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(GetCurrentProcess()) };
    let before = handle_count(&process);
    for _ in 0..16 {
        assert!(transfer(&mut captured, process).is_err());
        assert_eq!(captured, original);
        assert_eq!(handle_count(&process), before);
    }
}

#[test]
fn transferred_events_keep_identity_and_retry_uses_original_sources() {
    use windows_sys::Win32::{
        Foundation::WAIT_OBJECT_0,
        System::Threading::{SetEvent, WaitForSingleObject},
    };
    let source = event();
    let mut captured = capture(|| {
        encode(source.as_raw_handle() as u64);
        Ok(frame(&[]))
    })
    .unwrap();
    let process = unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(GetCurrentProcess()) };
    let before = handle_count(&process);
    for _ in 0..2 {
        transfer(&mut captured, process).unwrap();
        let destination = {
            let _scope = restore_scope(&captured).unwrap();
            unsafe { OwnedHandle::from_raw_handle(decode(TOKEN | 1).unwrap() as _) }
        };
        assert_ne!(source.as_raw_handle(), destination.as_raw_handle());
        assert_ne!(unsafe { SetEvent(destination.as_raw_handle()) }, 0);
        assert_eq!(
            unsafe { WaitForSingleObject(source.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        drop(destination);
        assert_eq!(handle_count(&process), before);
    }
}

fn command(role: &str) -> std::process::Command {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--exact",
        "native_transfer::tests::process_fixture",
        "--nocapture",
        "--test-threads=1",
    ]);
    command.env("KINAKAZE_TRANSFER_TEST_ROLE", role);
    command
}

fn wait(child: &mut std::process::Child) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "native transfer fixture: {status}");
            return;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("native transfer fixture exceeded deadline");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn cross_process_vfs_handoff_preserves_open_descriptions_and_ipc() {
    // Keep process-global descriptor/namespace state separate from other tests.
    let mut child = command("source")
        .stdin(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait(&mut child);
}

#[test]
fn process_fixture() {
    match std::env::var("KINAKAZE_TRANSFER_TEST_ROLE").as_deref() {
        Ok("source") => source_process(),
        Ok("destination") => destination_process(),
        _ => (),
    }
}

fn source_process() {
    use crate::{FdFlags as F, FdKind};
    use std::os::windows::fs::OpenOptionsExt;
    let path = std::env::temp_dir().join(format!(
        "kinakaze-native-transfer-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"abcdef").unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED)
        .open(&path)
        .unwrap();
    let alias = file.try_clone().unwrap();
    let flags = F::OVERLAPPED
        .union(F::SEEKABLE)
        .union(F::READ_ACCESS)
        .union(F::WRITE_ACCESS);
    let fd = crate::install(file.into_raw_handle() as usize, FdKind::File, flags).unwrap();
    let alias = crate::install_duplicate(
        alias.into_raw_handle() as usize,
        FdKind::File,
        flags,
        crate::get(fd).unwrap(),
    )
    .unwrap();
    let mut first = [0];
    assert_eq!(crate::read(fd, &mut first), Ok(1));
    assert_eq!(&first, b"a");
    let (read, write) = crate::create_pipe(F::NONE, 4096).unwrap();
    let (left, right) = crate::unix::socketpair(crate::socket::SOCK_STREAM).unwrap();
    let event = crate::eventfd::create_eventfd(7, crate::eventfd::EFD_NONBLOCK).unwrap();
    let fifo_path = path.with_extension("fifo");
    let marker = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&fifo_path)
        .unwrap();
    crate::fs::set_mode_host_path(&fifo_path, crate::fs::S_IFIFO | 0o600).unwrap();
    let fifo = crate::fifo::open_marker(
        marker.as_raw_handle(),
        crate::fs::O_RDWR | crate::fs::O_NONBLOCK,
    )
    .unwrap();
    let udp = crate::socket::socket(crate::socket::AF_INET, crate::socket::SOCK_DGRAM, 0).unwrap();
    let mut address = [0u8; 16];
    address[..2].copy_from_slice(&(crate::socket::AF_INET as u16).to_ne_bytes());
    address[4..8].copy_from_slice(&[127, 0, 0, 1]);
    unsafe { crate::socket::bind(udp, address.as_ptr(), address.len() as i32) }.unwrap();
    let mut address_length = address.len() as i32;
    unsafe { crate::socket::getsockname(udp, address.as_mut_ptr(), &mut address_length) }.unwrap();
    let port = u16::from_be_bytes(address[2..4].try_into().unwrap());
    assert_ne!(port, 0);
    assert_eq!(crate::write(write, b"pipe"), Ok(4));
    assert_eq!(crate::write(left, b"unix"), Ok(4));
    assert_eq!(crate::write(fifo, b"fifo"), Ok(4));
    let ids = [fd, alias, read, write, left, right, event, fifo, udp];
    crate::socket::prepare_process_fork().unwrap();
    let mut captured = crate::serialize_portable_fork_state().unwrap();
    let mut child = command("destination")
        .env("KINAKAZE_TRANSFER_TEST_PORT", port.to_string())
        .env(
            "KINAKAZE_TRANSFER_TEST_FDS",
            ids.map(|fd| fd.to_string()).join(","),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // The receiver parks at this pipe read before touching the transferred VFS.
    // Use enough independent native objects to exercise a different handle table.
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    use std::io::BufRead;
    loop {
        let mut line = String::new();
        assert_ne!(
            stdout.read_line(&mut line).unwrap(),
            0,
            "receiver exited before ready"
        );
        if line.contains("TRANSFER_READY") {
            break;
        }
    }
    transfer(&mut captured, child.as_handle()).unwrap();
    let rows = entries(&captured[table_range(&captured).unwrap().unwrap()]).unwrap();
    assert!(
        rows.iter()
            .any(|(source, destination)| source != destination)
    );
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(&(captured.len() as u32).to_le_bytes())
        .unwrap();
    stdin.write_all(&captured).unwrap();
    drop(stdin);
    crate::socket::finish_process_fork(child.id() as i32);
    wait(&mut child);
    assert_eq!(crate::read(alias, &mut first), Ok(1));
    assert_eq!(
        &first, b"d",
        "child advanced the shared open-description offset"
    );
    let mut counter = [0; 8];
    assert_eq!(crate::read(event, &mut counter), Err(crate::EAGAIN));
    for fd in ids {
        crate::close(fd).unwrap();
    }
    drop(marker);
    std::fs::remove_file(fifo_path).unwrap();
    std::fs::remove_file(path).unwrap();
}

fn destination_process() {
    let _unrelated: Vec<_> = (0..128).map(|_| event()).collect();
    println!("TRANSFER_READY");
    std::io::stdout().flush().unwrap();
    let mut input = std::io::stdin().lock();
    let mut length = [0; 4];
    input.read_exact(&mut length).unwrap();
    let mut captured = vec![0; u32::from_le_bytes(length) as usize];
    input.read_exact(&mut captured).unwrap();
    drop(input);
    let ids: Vec<i32> = std::env::var("KINAKAZE_TRANSFER_TEST_FDS")
        .unwrap()
        .split(',')
        .map(|fd| fd.parse().unwrap())
        .collect();
    assert!(crate::restore_fork_state(&captured));
    let mut file = [0; 2];
    assert_eq!(crate::read(ids[0], &mut file), Ok(2));
    assert_eq!(&file, b"bc");
    assert_eq!(
        crate::get(ids[0]).unwrap().description_id,
        crate::get(ids[1]).unwrap().description_id
    );
    let mut data = [0; 4];
    assert_eq!(crate::read(ids[2], &mut data), Ok(4));
    assert_eq!(&data, b"pipe");
    assert_eq!(crate::read(ids[5], &mut data), Ok(4));
    assert_eq!(&data, b"unix");
    let mut counter = [0; 8];
    assert_eq!(crate::read(ids[6], &mut counter), Ok(8));
    assert_eq!(u64::from_ne_bytes(counter), 7);
    assert_eq!(crate::read(ids[7], &mut data), Ok(4));
    assert_eq!(&data, b"fifo");
    let mut address = [0u8; 16];
    let mut length = address.len() as i32;
    unsafe { crate::socket::getsockname(ids[8], address.as_mut_ptr(), &mut length) }.unwrap();
    let port = u16::from_be_bytes(address[2..4].try_into().unwrap());
    assert_eq!(
        port.to_string(),
        std::env::var("KINAKAZE_TRANSFER_TEST_PORT").unwrap()
    );
    for fd in ids {
        crate::close(fd).unwrap();
    }
}
