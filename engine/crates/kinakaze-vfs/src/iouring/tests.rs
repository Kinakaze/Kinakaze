use super::*;
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("kinakaze-ring-{}-{nonce}", std::process::id()));
        std::fs::write(&path, vec![0x51; 16_013]).unwrap();
        Self(path)
    }
    fn open(&self, flags: i32) -> Fd {
        Fd(crate::fs::open(&crate::path::to_guest_path(&self.0), flags, 0).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
struct Fd(i32);
impl Drop for Fd {
    fn drop(&mut self) {
        crate::close(self.0).unwrap();
    }
}
fn completion(ring: &Fd, submission: &SubmissionEntry) -> CompletionEntry {
    unsafe { push(ring.0, submission) }.unwrap();
    assert_eq!(enter(ring.0, 1, Some(1000)).unwrap(), 1);
    let mut out = [CompletionEntry::default(); 2];
    assert_eq!(reap(ring.0, &mut out).unwrap(), 1);
    out[0]
}
#[test]
fn ordinary_native_reads_preserve_zero_and_maximum_guest_cookies() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    let file = f.open(crate::fs::O_RDONLY);
    let ring = Fd(setup(4, 0).unwrap());
    let installed = crate::get(ring.0).unwrap();
    assert_eq!(installed.kind, FdKind::IoRing);
    assert!(installed.flags.contains(FdFlags::CLOSE_ON_EXEC));
    assert!(installed.flags.contains(FdFlags::READ_ACCESS));
    assert!(installed.flags.contains(FdFlags::WRITE_ACCESS));
    assert!(opcode_supported(ring.0, IORING_OP_READ).unwrap());
    for user_data in [0, u64::MAX, 0] {
        let mut bytes = [0u8; 32];
        let entry = SubmissionEntry {
            opcode: IORING_OP_READ,
            fd: file.0,
            address: bytes.as_mut_ptr() as u64,
            length: bytes.len() as u32,
            user_data,
            ..SubmissionEntry::default()
        };
        let result = completion(&ring, &entry);
        assert_eq!(result.user_data, user_data);
        assert_eq!(result.result, bytes.len() as i32);
        assert_eq!(bytes, [0x51; 32]);
    }
}
#[test]
fn verity_native_reads_fail_without_touching_guest_buffers() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    let file = f.open(crate::fs::O_RDONLY);
    crate::fs::verity::enable(file.0, 1, 4096, b"ring").unwrap();
    let ring = Fd(setup(8, 0).unwrap());
    for opcode in [IORING_OP_READ, IORING_OP_READ_FIXED, IORING_OP_READV] {
        let mut bytes = [0xccu8; 32];
        let segment = IoVec {
            base: bytes.as_mut_ptr(),
            length: bytes.len(),
        };
        let vectored = opcode == IORING_OP_READV;
        let entry = SubmissionEntry {
            opcode,
            fd: file.0,
            address: if vectored {
                &segment as *const IoVec as u64
            } else {
                bytes.as_mut_ptr() as u64
            },
            length: if vectored { 1 } else { bytes.len() as u32 },
            ..SubmissionEntry::default()
        };
        let result = completion(&ring, &entry);
        assert_eq!(result.user_data, 0);
        assert_eq!(result.result, -crate::EOPNOTSUPP);
        assert_eq!(bytes, [0xcc; 32]);
    }
    let native_writer = Object::open(&f.0, windows_sys::Win32::Foundation::GENERIC_WRITE).unwrap();
    let writer = Fd(crate::install(
        native_writer.into_raw() as usize,
        FdKind::File,
        FdFlags::OVERLAPPED.union(FdFlags::SEEKABLE),
    )
    .unwrap());
    let payload = [0xeeu8; 32];
    let entry = SubmissionEntry {
        opcode: IORING_OP_WRITE,
        fd: writer.0,
        address: payload.as_ptr() as u64,
        length: payload.len() as u32,
        user_data: 7,
        ..SubmissionEntry::default()
    };
    assert_eq!(completion(&ring, &entry).result, -crate::EPERM);
    assert_eq!(
        completion(
            &ring,
            &SubmissionEntry {
                fd: file.0,
                ..entry
            }
        )
        .result,
        -EBADF
    );
    let mut verified = [0; 32];
    assert_eq!(crate::read(file.0, &mut verified).unwrap(), verified.len());
    assert_eq!(verified, [0x51; 32]);
}

#[test]
fn vectored_short_reads_are_ordered_aggregated_and_stop_at_eof() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    std::fs::write(&f.0, b"abcdefghij").unwrap();
    let file = f.open(crate::fs::O_RDONLY);
    let ring = Fd(setup(2, 0).unwrap());
    let mut first = [0xccu8; 4];
    let mut middle = [0xccu8; 4];
    let mut last = [0xccu8; 4];
    let segments = [
        IoVec {
            base: first.as_mut_ptr(),
            length: 4,
        },
        IoVec {
            base: middle.as_mut_ptr(),
            length: 4,
        },
        IoVec {
            base: last.as_mut_ptr(),
            length: 4,
        },
    ];
    let entry = SubmissionEntry {
        opcode: IORING_OP_READV,
        fd: file.0,
        address: segments.as_ptr() as u64,
        length: 3,
        user_data: 0,
        ..SubmissionEntry::default()
    };
    assert_eq!(completion(&ring, &entry).result, 10);
    assert_eq!(&first, b"abcd");
    assert_eq!(&middle, b"efgh");
    assert_eq!(&last, b"ij\xcc\xcc");
    first.fill(0xcc);
    middle.fill(0xcc);
    last.fill(0xcc);
    assert_eq!(
        completion(&ring, &SubmissionEntry { offset: 9, ..entry }).result,
        1
    );
    assert_eq!(&first, b"j\xcc\xcc\xcc");
    assert_eq!(middle, [0xcc; 4]);
    assert_eq!(last, [0xcc; 4]);
    first.fill(0xcc);
    assert_eq!(
        completion(
            &ring,
            &SubmissionEntry {
                offset: 10,
                ..entry
            }
        )
        .result,
        0
    );
    assert_eq!(first, [0xcc; 4]);
}

#[test]
fn vectored_writes_are_one_native_operation_and_honour_append() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    std::fs::write(&f.0, b"head").unwrap();
    let file = f.open(crate::fs::O_WRONLY | crate::fs::O_APPEND);
    let ring = Fd(setup(2, 0).unwrap());
    if !opcode_supported(ring.0, IORING_OP_WRITEV).unwrap() {
        return;
    }
    let first = *b"one";
    let second = *b"two";
    let segments = [
        IoVec {
            base: first.as_ptr().cast_mut(),
            length: 3,
        },
        IoVec {
            base: std::ptr::null_mut(),
            length: 0,
        },
        IoVec {
            base: second.as_ptr().cast_mut(),
            length: 3,
        },
    ];
    let entry = SubmissionEntry {
        opcode: IORING_OP_WRITEV,
        fd: file.0,
        offset: 0,
        address: segments.as_ptr() as u64,
        length: 3,
        user_data: u64::MAX,
        ..SubmissionEntry::default()
    };
    unsafe { push(ring.0, &entry) }.unwrap();
    assert_eq!(
        lookup(ring.0).unwrap().state.lock().unwrap().requests.len(),
        1
    );
    assert_eq!(enter(ring.0, 1, Some(1000)).unwrap(), 1);
    let mut out = [CompletionEntry::default()];
    assert_eq!(reap(ring.0, &mut out).unwrap(), 1);
    assert_eq!(out[0].result, 6);
    assert_eq!(out[0].user_data, u64::MAX);
    assert_eq!(std::fs::read(&f.0).unwrap(), b"headonetwo");
}

#[test]
fn queued_file_is_pinned_across_close_and_exact_fd_reuse() {
    if !available() {
        return;
    }
    let original = Fixture::new();
    let replacement = Fixture::new();
    std::fs::write(&replacement.0, vec![0xa5; 16_013]).unwrap();
    let flags = FdFlags::OVERLAPPED.union(FdFlags::SEEKABLE);
    let native = Object::open(&original.0, windows_sys::Win32::Foundation::GENERIC_READ).unwrap();
    let old =
        crate::install_at_least(native.into_raw() as usize, FdKind::File, flags, 300).unwrap();
    let ring = Fd(setup(2, 0).unwrap());
    let mut bytes = [0xccu8; 32];
    unsafe {
        push(
            ring.0,
            &SubmissionEntry {
                opcode: IORING_OP_READ,
                fd: old,
                address: bytes.as_mut_ptr() as u64,
                length: 32,
                ..SubmissionEntry::default()
            },
        )
    }
    .unwrap();
    crate::close(old).unwrap();
    let native =
        Object::open(&replacement.0, windows_sys::Win32::Foundation::GENERIC_READ).unwrap();
    let replacement_fd =
        Fd(crate::install_exact(native.into_raw() as usize, FdKind::File, flags, old).unwrap());
    assert_eq!(enter(ring.0, 1, Some(1000)).unwrap(), 1);
    let mut out = [CompletionEntry::default()];
    assert_eq!(reap(ring.0, &mut out).unwrap(), 1);
    assert_eq!(out[0].result, 32);
    assert_eq!(bytes, [0x51; 32]);
    let mut other = [0; 32];
    crate::read(replacement_fd.0, &mut other).unwrap();
    assert_eq!(other, [0xa5; 32]);
}

#[test]
fn closing_ring_wakes_all_enter_waiters_and_old_arcs_stay_closed() {
    if !available() {
        return;
    }
    let fd = setup(2, 0).unwrap();
    let old = lookup(fd).unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let mut threads = Vec::new();
    for _ in 0..4 {
        let send = send.clone();
        threads.push(std::thread::spawn(move || {
            send.send(enter(fd, 1, Some(5000))).unwrap();
        }));
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    while old.state.lock().unwrap().waiters.len() < 4 {
        assert!(Instant::now() < deadline, "enter did not register its wait");
        std::thread::yield_now();
    }
    crate::close(fd).unwrap();
    let replacement = Fd(setup(2, 0).unwrap());
    for _ in 0..4 {
        assert_eq!(
            receive
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            Err(EBADF)
        );
    }
    for thread in threads {
        thread.join().unwrap();
    }
    assert!(old.state.lock().unwrap().closing);
    assert_eq!(old.state.lock().unwrap().handle, 0);
    assert_eq!(
        completion(&replacement, &SubmissionEntry::default()).result,
        0
    );
}

#[test]
fn close_retires_submitted_private_buffers_without_publishing_them() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    let file = f.open(crate::fs::O_RDONLY);
    let fd = setup(4, 0).unwrap();
    let owner = lookup(fd).unwrap();
    let mut bytes = [0xccu8; 1024];
    unsafe {
        push(
            fd,
            &SubmissionEntry {
                opcode: IORING_OP_READ,
                fd: file.0,
                address: bytes.as_mut_ptr() as u64,
                length: 1024,
                ..SubmissionEntry::default()
            },
        )
    }
    .unwrap();
    {
        let mut state = owner.state.lock().unwrap();
        assert_eq!(state.submit().unwrap(), 1);
        assert_eq!(state.requests.len(), 1);
        assert!(state.requests.values().all(|request| request.submitted));
    }
    crate::close(fd).unwrap();
    assert_eq!(bytes, [0xcc; 1024]);
    assert!(owner.state.lock().unwrap().requests.is_empty());
    assert_eq!(owner.state.lock().unwrap().handle, 0);
}

#[test]
fn queued_writer_excludes_enable_until_unsubmitted_ring_is_closed() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    let writer = f.open(crate::fs::O_WRONLY);
    let reader = f.open(crate::fs::O_RDONLY);
    let ring = setup(2, 0).unwrap();
    if !opcode_supported(ring, IORING_OP_WRITE).unwrap() {
        crate::close(ring).unwrap();
        return;
    }
    let payload = [0xeeu8; 32];
    unsafe {
        push(
            ring,
            &SubmissionEntry {
                opcode: IORING_OP_WRITE,
                fd: writer.0,
                address: payload.as_ptr() as u64,
                length: 32,
                ..SubmissionEntry::default()
            },
        )
    }
    .unwrap();
    drop(writer);
    assert_eq!(
        crate::fs::verity::enable(reader.0, 1, 4096, b"lease"),
        Err(26)
    );
    crate::close(ring).unwrap();
    crate::fs::verity::enable(reader.0, 1, 4096, b"lease").unwrap();
    let mut bytes = [0; 32];
    crate::read(reader.0, &mut bytes).unwrap();
    assert_eq!(bytes, [0x51; 32]);
}

#[test]
fn read_queued_before_enable_never_publishes_even_after_native_completion() {
    if !available() {
        return;
    }
    for submitted in [false, true] {
        let f = Fixture::new();
        let file = f.open(crate::fs::O_RDONLY);
        let ring = Fd(setup(2, 0).unwrap());
        let mut bytes = [0xccu8; 128];
        unsafe {
            push(
                ring.0,
                &SubmissionEntry {
                    opcode: IORING_OP_READ,
                    fd: file.0,
                    address: bytes.as_mut_ptr() as u64,
                    length: 128,
                    ..SubmissionEntry::default()
                },
            )
        }
        .unwrap();
        if submitted {
            assert_eq!(
                lookup(ring.0)
                    .unwrap()
                    .state
                    .lock()
                    .unwrap()
                    .submit()
                    .unwrap(),
                1
            );
        }
        crate::fs::verity::enable(file.0, 1, 4096, b"race").unwrap();
        assert_eq!(enter(ring.0, 1, Some(1000)).unwrap(), u32::from(!submitted));
        let mut out = [CompletionEntry::default()];
        assert_eq!(reap(ring.0, &mut out).unwrap(), 1);
        assert_eq!(out[0].result, -crate::EOPNOTSUPP);
        assert_eq!(bytes, [0xcc; 128]);
    }
}

#[test]
#[ignore = "known P1: default-stream Merkle tail can escape after abort and regrowth"]
fn aborted_verity_then_regrowth_must_not_publish_a_completed_merkle_tail_read() {
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    assert!(
        available(),
        "this regression requires native IoRing support"
    );
    let f = Fixture::new();
    let file = f.open(crate::fs::O_RDONLY);
    let ring = Fd(setup(2, 0).unwrap());
    let mut guest = [0xccu8; 64];
    unsafe {
        push(
            ring.0,
            &SubmissionEntry {
                opcode: IORING_OP_READ,
                fd: file.0,
                offset: 16_384,
                address: guest.as_mut_ptr() as u64,
                length: guest.len() as u32,
                ..SubmissionEntry::default()
            },
        )
    }
    .unwrap();
    let opened = crate::fs::verity::Opened::from_fd(file.0).unwrap();
    let mut transaction = opened.prepare(1, 4096, b"abort-regrowth").unwrap();
    transaction.build().unwrap();
    let native = lookup(ring.0).unwrap();
    assert_eq!(native.state.lock().unwrap().submit().unwrap(), 1);
    assert_eq!(
        unsafe { WaitForSingleObject(native.completion_event.raw(), 2_000) },
        WAIT_OBJECT_0
    );
    let tree_bytes = {
        let state = native.state.lock().unwrap();
        // The completion event proves the native operation has stopped writing;
        // leave its CQE and Request intact until after the rollback and regrowth.
        state.requests.values().next().unwrap().buffer.clone()
    };
    assert_ne!(
        tree_bytes,
        vec![0; 64],
        "the fixture must actually contain a tree block"
    );
    assert_eq!(guest, [0xcc; 64], "native completion must still be private");
    transaction.abort().unwrap();
    drop(transaction);
    let writer = Object::open(&f.0, GENERIC_READ | GENERIC_WRITE).unwrap();
    assert_eq!(writer.write_at(16_013, &[0x72; 8192]).unwrap(), 8192);
    writer.flush().unwrap();
    drop(writer);
    assert_eq!(
        crate::fs::verity::logical_size(opened.borrowed_handle()).unwrap(),
        None
    );
    let mut out = [CompletionEntry::default()];
    assert_eq!(reap(ring.0, &mut out).unwrap(), 1);
    eprintln!(
        "after abort/regrowth CQE={} private-tree-published={}",
        out[0].result,
        guest.as_slice() == tree_bytes.as_slice()
    );
    assert_ne!(
        guest.as_slice(),
        tree_bytes.as_slice(),
        "a finished read of temporary Merkle storage must never become ordinary file data"
    );
}

struct Page(usize);
impl Page {
    fn new() -> Self {
        use windows_sys::Win32::System::Memory::*;
        let pointer = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                4096,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        assert!(!pointer.is_null());
        unsafe { std::ptr::write_bytes(pointer.cast::<u8>(), 0xcc, 4096) };
        Self(pointer as usize)
    }
    fn protect(&self, protection: u32) {
        let mut old = 0;
        assert_ne!(
            unsafe {
                windows_sys::Win32::System::Memory::VirtualProtect(
                    self.0 as *const _,
                    4096,
                    protection,
                    &mut old,
                )
            },
            0
        );
    }
}
impl Drop for Page {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Memory::VirtualFree(
                self.0 as *mut _,
                0,
                windows_sys::Win32::System::Memory::MEM_RELEASE,
            )
        };
    }
}

#[test]
fn invalid_iovecs_and_readonly_or_unmapped_outputs_are_efault_not_host_faults() {
    if !available() {
        return;
    }
    use windows_sys::Win32::System::Memory::{PAGE_NOACCESS, PAGE_READONLY};
    let f = Fixture::new();
    let file = f.open(crate::fs::O_RDWR);
    let ring = Fd(setup(4, 0).unwrap());
    for opcode in [
        IORING_OP_READV,
        IORING_OP_WRITEV,
        IORING_OP_READ,
        IORING_OP_WRITE,
    ] {
        let entry = SubmissionEntry {
            opcode,
            fd: file.0,
            address: 1,
            length: 1,
            ..SubmissionEntry::default()
        };
        assert_eq!(completion(&ring, &entry).result, -crate::EFAULT);
    }
    for protection in [PAGE_READONLY, PAGE_NOACCESS] {
        let page = Page::new();
        let entry = SubmissionEntry {
            opcode: IORING_OP_READ,
            fd: file.0,
            address: page.0 as u64,
            length: 32,
            ..SubmissionEntry::default()
        };
        unsafe { push(ring.0, &entry) }.unwrap();
        page.protect(protection);
        enter(ring.0, 1, Some(1000)).unwrap();
        let mut out = [CompletionEntry::default()];
        reap(ring.0, &mut out).unwrap();
        assert_eq!(out[0].result, -crate::EFAULT);
        page.protect(PAGE_READONLY);
        assert_eq!(unsafe { *(page.0 as *const u8) }, 0xcc);
    }
    let page = Page::new();
    unsafe {
        push(
            ring.0,
            &SubmissionEntry {
                opcode: IORING_OP_READ,
                fd: file.0,
                address: page.0 as u64,
                length: 32,
                ..SubmissionEntry::default()
            },
        )
    }
    .unwrap();
    drop(page);
    enter(ring.0, 1, Some(1000)).unwrap();
    let mut out = [CompletionEntry::default()];
    reap(ring.0, &mut out).unwrap();
    assert_eq!(out[0].result, -crate::EFAULT);
}

#[test]
fn later_bad_vector_segments_return_the_transferred_prefix() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    let file = f.open(crate::fs::O_RDWR);
    let ring = Fd(setup(2, 0).unwrap());
    let mut first = [0xccu8; 4];
    let vectors = [
        IoVec {
            base: first.as_mut_ptr(),
            length: 4,
        },
        IoVec {
            base: 1 as *mut u8,
            length: 4,
        },
    ];
    let entry = SubmissionEntry {
        opcode: IORING_OP_READV,
        fd: file.0,
        address: vectors.as_ptr() as u64,
        length: 2,
        ..SubmissionEntry::default()
    };
    assert_eq!(completion(&ring, &entry).result, 4);
    assert_eq!(first, [0x51; 4]);
    first.fill(0xa5);
    assert_eq!(
        completion(
            &ring,
            &SubmissionEntry {
                opcode: IORING_OP_WRITEV,
                ..entry
            }
        )
        .result,
        4
    );
    let disk = std::fs::read(&f.0).unwrap();
    assert_eq!(
        &disk[..8],
        &[0xa5, 0xa5, 0xa5, 0xa5, 0x51, 0x51, 0x51, 0x51]
    );
}

#[test]
fn fault_inside_one_cross_page_buffer_reports_the_accessible_prefix() {
    if !available() {
        return;
    }
    use windows_sys::Win32::System::Memory::*;
    let f = Fixture::new();
    let file = f.open(crate::fs::O_RDWR);
    let ring = Fd(setup(2, 0).unwrap());
    let address = unsafe {
        VirtualAlloc(
            std::ptr::null(),
            8192,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    assert!(!address.is_null());
    let allocation = Page(address as usize);
    unsafe { std::ptr::write_bytes(address.cast::<u8>(), 0xcc, 8192) };
    let mut old = 0;
    assert_ne!(
        unsafe {
            VirtualProtect(
                (allocation.0 + 4096) as *const _,
                4096,
                PAGE_NOACCESS,
                &mut old,
            )
        },
        0
    );
    let entry = SubmissionEntry {
        opcode: IORING_OP_READ,
        fd: file.0,
        address: (allocation.0 + 4080) as u64,
        length: 32,
        ..SubmissionEntry::default()
    };
    assert_eq!(completion(&ring, &entry).result, 16);
    assert_eq!(
        unsafe { std::slice::from_raw_parts((allocation.0 + 4080) as *const u8, 16) },
        &[0x51; 16]
    );
    unsafe { std::ptr::write_bytes((allocation.0 + 4080) as *mut u8, 0xa5, 16) };
    assert_eq!(
        completion(
            &ring,
            &SubmissionEntry {
                opcode: IORING_OP_WRITE,
                ..entry
            }
        )
        .result,
        16
    );
    let disk = std::fs::read(&f.0).unwrap();
    assert_eq!(&disk[..16], &[0xa5; 16]);
    assert_eq!(disk[16], 0x51);
}

#[test]
fn internal_cookie_wrap_and_repeated_guest_zero_cookies_do_not_alias() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    let file = f.open(crate::fs::O_RDONLY);
    let ring = Fd(setup(4, 0).unwrap());
    lookup(ring.0).unwrap().state.lock().unwrap().next_cookie = usize::MAX;
    let mut bytes = [[0xccu8; 4]; 3];
    for target in &mut bytes {
        unsafe {
            push(
                ring.0,
                &SubmissionEntry {
                    opcode: IORING_OP_READ,
                    fd: file.0,
                    address: target.as_mut_ptr() as u64,
                    length: 4,
                    user_data: 0,
                    ..SubmissionEntry::default()
                },
            )
        }
        .unwrap();
    }
    assert_eq!(enter(ring.0, 3, Some(1000)).unwrap(), 3);
    let mut out = [CompletionEntry::default(); 3];
    assert_eq!(reap(ring.0, &mut out).unwrap(), 3);
    assert!(
        out.iter()
            .all(|completion| completion.user_data == 0 && completion.result == 4)
    );
    assert_eq!(bytes, [[0x51; 4]; 3]);
}

#[test]
fn waiting_for_two_completions_does_not_finish_on_only_one() {
    if !available() {
        return;
    }
    let ring = Fd(setup(4, 0).unwrap());
    let owner = lookup(ring.0).unwrap();
    unsafe { push(ring.0, &SubmissionEntry::default()) }.unwrap();
    let fd = ring.0;
    let (send, receive) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        send.send(enter(fd, 2, Some(2000))).unwrap();
    });
    let deadline = Instant::now() + std::time::Duration::from_secs(1);
    while owner.state.lock().unwrap().waiters.is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(receive.try_recv().is_err());
    unsafe {
        push(
            ring.0,
            &SubmissionEntry {
                user_data: 1,
                ..SubmissionEntry::default()
            },
        )
    }
    .unwrap();
    assert_eq!(enter(ring.0, 0, None).unwrap(), 1);
    assert_eq!(
        receive
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap(),
        Ok(1)
    );
    waiter.join().unwrap();
    let mut out = [CompletionEntry::default(); 2];
    assert_eq!(reap(ring.0, &mut out).unwrap(), 2);
}

#[test]
fn interrupted_wait_reports_already_consumed_entries_without_releasing_ring_state() {
    if !available() {
        return;
    }
    for submitted in [false, true] {
        let ring = Fd(setup(2, 0).unwrap());
        let owner = lookup(ring.0).unwrap();
        if submitted {
            unsafe { push(ring.0, &SubmissionEntry::default()) }.unwrap();
        }
        let fd = ring.0;
        let (send, receive) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            assert!(!interrupt::current().is_null());
            send.send(interrupt::current_thread_id()).unwrap();
            enter(fd, 2, Some(2000))
        });
        let thread_id = receive.recv().unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(1);
        while owner.state.lock().unwrap().waiters.is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(interrupt::interrupt_thread(thread_id));
        assert_eq!(
            thread.join().unwrap(),
            if submitted { Ok(1) } else { Err(crate::EINTR) }
        );
        assert!(!owner.state.lock().unwrap().closing);
        if submitted {
            let mut out = [CompletionEntry::default()];
            assert_eq!(reap(fd, &mut out).unwrap(), 1);
            assert_eq!(out[0].result, 0);
        }
    }
}

// Fork/exec serializers inspect process-global descriptor state. Run their race
// scenarios in a short-lived child so unrelated parallel tests cannot add live
// rings (or borrowed test descriptors) to that snapshot.
fn isolated(name: &str, scenario: impl FnOnce()) {
    const VARIABLE: &str = "KINAKAZE_IORING_CLASSIFICATION_TEST";
    if std::env::var(VARIABLE).as_deref() == Ok(name) {
        scenario();
        return;
    }
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture"])
        .env(VARIABLE, name)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // These children need no guest fd inheritance. Use the production spawn
    // filter so they cannot prolong another parallel test's writer lifetime.
    // Restore parent flags immediately after spawn, never after child exit.
    let child = crate::with_exec_handle_filter(|| command.spawn())
        .unwrap()
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn delayed_ring_cleanup_cannot_misclassify_a_reused_event_descriptor() {
    isolated(
        "iouring::tests::delayed_ring_cleanup_cannot_misclassify_a_reused_event_descriptor",
        || {
            if !available() {
                return;
            }
            let fd = setup(2, 0).unwrap();
            let old = lookup(fd).unwrap();
            // Freeze precisely after close removed the slot but before it retires
            // its side table. The old placeholder stays live, as in real close.
            let removed = crate::table()
                .write()
                .unwrap()
                .slots
                .remove(fd as usize)
                .unwrap();
            let replacement = Event::new(true).unwrap();
            let raw = replacement.0;
            crate::install_exact(raw, FdKind::Event, FdFlags::NONE, fd).unwrap();
            std::mem::forget(replacement);
            assert_eq!(crate::get(fd).unwrap().kind, FdKind::Event);
            assert!(!is_ring(fd));
            assert!(matches!(lookup(fd), Err(EBADF)));
            assert_eq!(
                crate::procfs::local_fd_link_target(fd).unwrap(),
                "anon_inode:[eventpoll]"
            );
            assert!(
                crate::serialize_table().is_ok(),
                "stale ring registry must not reject an ordinary Event fork"
            );
            assert!(
                crate::serialize_exec_state(&[]).is_ok(),
                "stale ring registry must not reject Event exec"
            );
            crate::close(fd).unwrap();
            assert!(
                rings().lock().unwrap().contains_key(&fd),
                "closing an Event must not clean up an old IoRing"
            );
            forget_ring(fd, removed.raw);
            assert_eq!(old.state.lock().unwrap().handle, 0);
            assert_ne!(unsafe { CloseHandle(removed.raw as HANDLE) }, 0);
        },
    );
}

#[test]
fn ring_kind_survives_registry_removal_for_fork_exec_and_restore_checks() {
    isolated(
        "iouring::tests::ring_kind_survives_registry_removal_for_fork_exec_and_restore_checks",
        || {
            if !available() {
                return;
            }
            let ring = Fd(setup(2, 0).unwrap());
            let entry = crate::get(ring.0).unwrap();
            assert_eq!(entry.kind.fork_code(), 23);
            assert_eq!(FdKind::from_fork_code(23), FdKind::IoRing);
            assert_eq!(
                crate::procfs::local_fd_link_target(ring.0).unwrap(),
                "anon_inode:[io_uring]"
            );
            forget_ring(ring.0, entry.raw);
            assert!(is_ring(ring.0));
            assert_eq!(crate::serialize_table(), Err(crate::EOPNOTSUPP));
            assert!(
                crate::serialize_exec_state(&[]).is_ok(),
                "CLOEXEC rings are excluded from exec"
            );
            crate::set_close_on_exec(ring.0, false).unwrap();
            assert_eq!(crate::serialize_exec_state(&[]), Err(crate::EOPNOTSUPP));
            // A handoff record with a valid native placeholder is still not enough
            // to reconstruct a ring. Reject atomically, rather than restoring a lie.
            let mut wire = 1u32.to_le_bytes().to_vec();
            wire.extend_from_slice(&300u32.to_le_bytes());
            wire.extend_from_slice(&(entry.raw as u64).to_le_bytes());
            wire.extend_from_slice(&23u32.to_le_bytes());
            wire.extend_from_slice(&entry.flags.0.to_le_bytes());
            wire.extend_from_slice(&0u64.to_le_bytes());
            wire.extend_from_slice(&entry.description_id.to_le_bytes());
            assert!(crate::restore_table(&wire).is_err());
            assert!(crate::get(300).is_err());
        },
    );
}

#[test]
fn closing_source_cannot_turn_a_pinned_ring_duplicate_into_an_event() {
    isolated(
        "iouring::tests::closing_source_cannot_turn_a_pinned_ring_duplicate_into_an_event",
        || {
            if !available() {
                return;
            }
            use windows_sys::Win32::Foundation::{
                DUPLICATE_SAME_ACCESS, DuplicateHandle, GetHandleInformation,
            };
            use windows_sys::Win32::System::Threading::GetCurrentProcess;
            let fd = setup(2, 0).unwrap();
            let source = crate::get(fd).unwrap();
            let process = unsafe { GetCurrentProcess() };
            let mut duplicate = std::ptr::null_mut();
            assert_ne!(
                unsafe {
                    DuplicateHandle(
                        process,
                        source.raw as HANDLE,
                        process,
                        &mut duplicate,
                        0,
                        0,
                        DUPLICATE_SAME_ACCESS,
                    )
                },
                0
            );
            let pinned = Event(duplicate as usize);
            crate::close(fd).unwrap();
            assert!(!rings().lock().unwrap().contains_key(&fd));
            let replacement = Event::new(true).unwrap();
            crate::install_exact(replacement.0, FdKind::Event, FdFlags::NONE, fd).unwrap();
            std::mem::forget(replacement);
            let flags = FdFlags::READ_ACCESS.union(FdFlags::WRITE_ACCESS);
            for result in [
                crate::install_duplicate(pinned.0, source.kind, flags, source),
                crate::install_duplicate_at_least(pinned.0, source.kind, flags, 300, source),
                crate::install_duplicate_exact(pinned.0, source.kind, flags, 301, source),
            ] {
                assert_eq!(result, Err(crate::EOPNOTSUPP));
            }
            let mut native_flags = 0;
            assert_ne!(
                unsafe { GetHandleInformation(pinned.raw(), &mut native_flags) },
                0,
                "rejected dup leaves caller ownership intact"
            );
            assert!(crate::get_by_description_id(source.description_id).is_none());
            assert_eq!(crate::get(fd).unwrap().kind, FdKind::Event);
            crate::close(fd).unwrap();
        },
    );
}
