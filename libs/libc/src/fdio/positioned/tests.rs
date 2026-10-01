use super::*;
use kinakaze_vfs::fs::{self, SEEK_CUR, SEEK_SET};
use std::path::PathBuf;

struct Fixture {
    fd: i32,
    path: PathBuf,
}
impl Fixture {
    fn new(tag: &str, data: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "kinakaze-file-vector-{tag}-{}-{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, data).unwrap();
        let fd = fs::open(&super::super::windows_to_linux(&path), fs::O_RDWR, 0).unwrap();
        Self { fd, path }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = kinakaze_vfs::close(self.fd);
        let _ = std::fs::remove_file(&self.path);
    }
}
fn vector(bytes: &mut [u8]) -> IoVec {
    IoVec {
        iov_base: bytes.as_mut_ptr().cast(),
        iov_len: bytes.len(),
    }
}

#[test]
fn file_vectors_keep_shared_offsets_short_reads_and_partial_faults() {
    let file = Fixture::new("semantics", b"abcdefgh");
    let alias = super::super::duplicate(file.fd, false, super::super::Placement::Lowest).unwrap();
    let mut first = [0u8; 3];
    let mut second = [0u8; 8];
    let input = [
        vector(&mut first),
        IoVec {
            iov_base: core::ptr::null_mut(),
            iov_len: 0,
        },
        vector(&mut second),
    ];
    assert_eq!(unsafe { stream(file.fd, input.as_ptr(), 3, false) }, 8);
    assert_eq!(&first, b"abc");
    assert_eq!(&second[..5], b"defgh");
    assert_eq!(&second[5..], &[0; 3]);
    assert_eq!(fs::lseek(alias, 0, SEEK_CUR), Ok(8));

    fs::lseek(alias, 1, SEEK_SET).unwrap();
    let mut replacement = *b"XY";
    let partial = [
        vector(&mut replacement),
        IoVec {
            iov_base: core::ptr::null_mut(),
            iov_len: 1,
        },
    ];
    assert_eq!(unsafe { stream(file.fd, partial.as_ptr(), 2, true) }, 2);
    assert_eq!(fs::lseek(alias, 0, SEEK_CUR), Ok(3));
    assert_eq!(std::fs::read(&file.path).unwrap(), b"aXYdefgh");
    let fault = [
        IoVec {
            iov_base: core::ptr::null_mut(),
            iov_len: 1,
        },
        vector(&mut replacement),
    ];
    assert_eq!(unsafe { stream(file.fd, fault.as_ptr(), 2, false) }, -1);
    assert_eq!(kinakaze_tls::errno(), EFAULT);
    assert_eq!(fs::lseek(alias, 0, SEEK_CUR), Ok(3));
    let overflow = [
        IoVec {
            iov_base: replacement.as_mut_ptr().cast(),
            iov_len: isize::MAX as usize,
        },
        vector(&mut replacement),
    ];
    assert_eq!(unsafe { stream(file.fd, overflow.as_ptr(), 2, true) }, -1);
    assert_eq!(kinakaze_tls::errno(), EINVAL);
    assert_eq!(std::fs::read(&file.path).unwrap(), b"aXYdefgh");
    kinakaze_vfs::close(alias).unwrap();
}

#[test]
fn file_vectors_append_and_v2_stream_position() {
    let file = Fixture::new("append", b"start");
    kinakaze_vfs::set_status_flags(file.fd, true, false).unwrap();
    let mut first = *b"12";
    let mut second = *b"34";
    let output = [vector(&mut first), vector(&mut second)];
    assert_eq!(
        unsafe { kinakaze_abi_pwritev64v2(file.fd, output.as_ptr(), 2, -1, 0) },
        4
    );
    assert_eq!(fs::lseek(file.fd, 0, SEEK_CUR), Ok(9));
    assert_eq!(std::fs::read(&file.path).unwrap(), b"start1234");
    fs::lseek(file.fd, 5, SEEK_SET).unwrap();
    first.fill(0);
    second.fill(0);
    assert_eq!(
        unsafe { kinakaze_abi_preadv64v2(file.fd, output.as_ptr(), 2, -1, 0) },
        4
    );
    assert_eq!(&first, b"12");
    assert_eq!(&second, b"34");
    assert_eq!(fs::lseek(file.fd, 0, SEEK_CUR), Ok(9));
}

#[test]
fn file_vectors_retain_original_inode_after_close_and_fd_reuse() {
    let file = Fixture::new("lifetime", b"abcdefgh");
    let other = Fixture::new("replacement", b"untouched");
    let mut first = [0u8; 4];
    let mut second = [0u8; 4];
    let items = [vector(&mut first), vector(&mut second)];
    let mut replaced = false;
    assert_eq!(
        unsafe {
            kinakaze_vfs::file_vector::transfer(file.fd, 2, 8, false, |index| {
                if index == 1 && !replaced {
                    super::super::duplicate(
                        other.fd,
                        false,
                        super::super::Placement::Exactly(file.fd),
                    )
                    .unwrap();
                    replaced = true;
                }
                (items[index].iov_base.cast(), items[index].iov_len)
            })
        },
        Ok(8)
    );
    assert!(replaced);
    assert_eq!(&first, b"abcd");
    assert_eq!(&second, b"efgh");
    assert_eq!(fs::lseek(file.fd, 0, SEEK_CUR), Ok(0));
    assert_eq!(std::fs::read(&other.path).unwrap(), b"untouched");
}

#[test]
fn file_vectors_check_access_empty_vectors_and_read_only_reads() {
    let file = Fixture::new("access", b"abcdefgh");
    let guest = super::super::windows_to_linux(&file.path);
    let reader = fs::open(&guest, fs::O_RDONLY, 0).unwrap();
    let writer = fs::open(&guest, fs::O_WRONLY, 0).unwrap();
    let mut first = [0u8; 4];
    let mut second = [0u8; 4];
    let items = [vector(&mut first), vector(&mut second)];
    assert_eq!(unsafe { stream(reader, items.as_ptr(), 2, false) }, 8);
    assert_eq!(&first, b"abcd");
    assert_eq!(&second, b"efgh");
    for (fd, writing) in [(reader, true), (writer, false)] {
        assert_eq!(unsafe { stream(fd, items.as_ptr(), 2, writing) }, -1);
        assert_eq!(kinakaze_tls::errno(), EBADF);
    }
    assert_eq!(unsafe { stream(-1, core::ptr::null(), 0, false) }, -1);
    assert_eq!(kinakaze_tls::errno(), EBADF);
    assert_eq!(unsafe { stream(reader, core::ptr::null(), 0, false) }, 0);
    kinakaze_vfs::close(reader).unwrap();
    kinakaze_vfs::close(writer).unwrap();
}

#[test]
fn file_vectors_raw_syscalls_and_promoted_description() {
    let file = Fixture::new("raw", b"abcdefgh");
    let alias = super::super::duplicate(file.fd, false, super::super::Placement::Lowest).unwrap();
    drop(kinakaze_vfs::serialize_fork_state().unwrap());
    let mut first = [0u8; 4];
    let mut second = [0u8; 4];
    let items = [vector(&mut first), vector(&mut second)];
    for number in [19, 327] {
        fs::lseek(alias, 0, SEEK_SET).unwrap();
        assert_eq!(
            unsafe {
                crate::sysadmin::kinakaze_abi_syscall_raw(
                    number,
                    file.fd as u64,
                    items.as_ptr() as u64,
                    2,
                    if number == 327 { u64::MAX } else { 0 },
                    if number == 327 { u64::MAX } else { 0 },
                    0,
                )
            },
            8
        );
        assert_eq!(&first, b"abcd");
        assert_eq!(&second, b"efgh");
        assert_eq!(fs::lseek(alias, 0, SEEK_CUR), Ok(8));
    }
    first.copy_from_slice(b"1234");
    second.copy_from_slice(b"5678");
    for number in [20, 328] {
        fs::lseek(alias, 0, SEEK_SET).unwrap();
        assert_eq!(
            unsafe {
                crate::sysadmin::kinakaze_abi_syscall_raw(
                    number,
                    file.fd as u64,
                    items.as_ptr() as u64,
                    2,
                    if number == 328 { u64::MAX } else { 0 },
                    if number == 328 { u64::MAX } else { 0 },
                    0,
                )
            },
            8
        );
        assert_eq!(fs::lseek(alias, 0, SEEK_CUR), Ok(8));
        assert_eq!(std::fs::read(&file.path).unwrap(), b"12345678");
    }
    kinakaze_vfs::close(alias).unwrap();
}

#[test]
fn file_vectors_serialize_all_segments_against_dup_alias_io() {
    use std::sync::mpsc;
    use std::time::Duration;
    let file = Fixture::new("serialization", b"--------");
    let alias = super::super::duplicate(file.fd, false, super::super::Placement::Lowest).unwrap();
    let (parked_tx, parked_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let fd = file.fd;
    let vector_worker = std::thread::spawn(move || {
        let mut first = *b"abcd";
        let mut second = *b"efgh";
        let items = [vector(&mut first), vector(&mut second)];
        unsafe {
            kinakaze_vfs::file_vector::transfer(fd, 2, 8, true, |index| {
                if index == 1 {
                    parked_tx.send(()).unwrap();
                    resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                (items[index].iov_base.cast(), items[index].iov_len)
            })
        }
    });
    parked_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let alias_worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result = kinakaze_vfs::write(alias, b"Z");
        finished_tx.send(result).unwrap();
        kinakaze_vfs::close(alias).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let early_result = finished_rx.recv_timeout(Duration::from_millis(20));
    resume_tx.send(()).unwrap();
    assert_eq!(vector_worker.join().unwrap(), Ok(8));
    alias_worker.join().unwrap();
    assert_eq!(early_result, Err(mpsc::RecvTimeoutError::Timeout));
    assert_eq!(
        finished_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        Ok(1)
    );
    assert_eq!(std::fs::read(&file.path).unwrap(), b"abcdefghZ");
    assert_eq!(fs::lseek(file.fd, 0, SEEK_CUR), Ok(9));
}

#[test]
fn file_vectors_deliver_size_limit_signal_after_partial_write_and_unlock() {
    use core::sync::atomic::{AtomicI32, Ordering};
    use kinakaze_vfs::{limits, signal};
    static HANDLER_FD: AtomicI32 = AtomicI32::new(-1);
    static OBSERVED_OFFSET: AtomicI32 = AtomicI32::new(-1);
    unsafe extern "sysv64" fn handler(_signal: i32) {
        let offset = fs::lseek(HANDLER_FD.load(Ordering::SeqCst), 0, SEEK_CUR);
        OBSERVED_OFFSET.store(offset.unwrap_or(u64::MAX) as i32, Ordering::SeqCst);
    }
    let file = Fixture::new("limit", b"");
    HANDLER_FD.store(file.fd, Ordering::SeqCst);
    OBSERVED_OFFSET.store(-1, Ordering::SeqCst);
    let old_action = signal::sigaction(
        25,
        Some(signal::Action {
            disposition: signal::Disposition::Handle(handler, 0),
            ..Default::default()
        }),
    )
    .unwrap();
    let old_mask = signal::sigprocmask(signal::SIG_UNBLOCK, 1 << 24).unwrap();
    let old_limit = limits::get(0, limits::FSIZE).unwrap();
    limits::limits(0, limits::FSIZE, Some((4, old_limit.1))).unwrap();
    let mut first = *b"abcd";
    let mut second = *b"efgh";
    let items = [vector(&mut first), vector(&mut second)];
    let result = unsafe { stream(file.fd, items.as_ptr(), 2, true) };
    let observed = OBSERVED_OFFSET.load(Ordering::SeqCst);
    limits::limits(0, limits::FSIZE, Some(old_limit)).unwrap();
    signal::sigaction(25, Some(old_action)).unwrap();
    signal::sigprocmask(signal::SIG_SETMASK, old_mask).unwrap();
    assert_eq!(result, 4);
    assert_eq!(
        observed, 4,
        "handler can take the position lock after partial I/O"
    );
    assert_eq!(std::fs::read(&file.path).unwrap(), b"abcd");
}

#[test]
#[ignore = "paired release file readv/writev transaction benchmark"]
fn benchmark_file_vectors() {
    let file = Fixture::new("benchmark", &[0x5a; 2048]);
    let mut rows = Vec::new();
    // Separate arrays exercise actual scatter/gather buffers, no adjacency
    // coalescing or aggregate-copy shortcut can account for these timings.
    let mut payload = [[0x5au8; 128]; 32];
    let items: Vec<_> = payload
        .iter_mut()
        .map(|bytes| vector(&mut bytes[..64]))
        .collect();
    for shared in [false, true] {
        if shared {
            // SCM_RIGHTS/fork-promoted descriptions persist their position
            // through this same store. Keep promotion outside the timed body.
            drop(kinakaze_vfs::serialize_fork_state().unwrap());
        }
        for count in [1, 8, 32] {
            for writing in [false, true] {
                let began = std::time::Instant::now();
                for _ in 0..2000 {
                    fs::lseek(file.fd, 0, SEEK_SET).unwrap();
                    assert_eq!(
                        unsafe {
                            crate::sysadmin::kinakaze_abi_syscall_raw(
                                if writing { 20 } else { 19 },
                                file.fd as u64,
                                items.as_ptr() as u64,
                                count as u64,
                                0,
                                0,
                                0,
                            )
                        },
                        count as i64 * 64
                    );
                }
                rows.push(format!(
                    "\"{}_{}_{}_ns\":{}",
                    if shared { "shared" } else { "local" },
                    if writing { "writev" } else { "readv" },
                    count,
                    began.elapsed().as_nanos()
                ));
            }
        }
    }
    assert!(payload.iter().flatten().all(|byte| *byte == 0x5a));
    drop(file);
    // Measure throughput with gaps between buffers and 128 MiB moved per row.
    // Native file caching is enabled; these are not fsync/device-flush timings.
    let bulk = Fixture::new("bulk", &vec![0x5a; 8 * 1024 * 1024]);
    for shared in [false, true] {
        if shared {
            drop(kinakaze_vfs::serialize_fork_state().unwrap());
        }
        for (count, length) in [(1024, 4096), (32, 65536), (8, 1048576)] {
            let mut payload: Vec<_> = (0..count).map(|_| vec![0x5au8; length + 64]).collect();
            let items: Vec<_> = payload
                .iter_mut()
                .map(|bytes| vector(&mut bytes[..length]))
                .collect();
            let iterations = 128 * 1024 * 1024 / (count * length);
            for writing in [false, true] {
                let began = std::time::Instant::now();
                for _ in 0..iterations {
                    fs::lseek(bulk.fd, 0, SEEK_SET).unwrap();
                    assert_eq!(
                        unsafe {
                            crate::sysadmin::kinakaze_abi_syscall_raw(
                                if writing { 20 } else { 19 },
                                bulk.fd as u64,
                                items.as_ptr() as u64,
                                count as u64,
                                0,
                                0,
                                0,
                            )
                        },
                        (count * length) as i64
                    );
                }
                rows.push(format!(
                    "\"bulk_{}_{}_{}x{}_ns\":{}",
                    if shared { "shared" } else { "local" },
                    if writing { "writev" } else { "readv" },
                    count,
                    length,
                    began.elapsed().as_nanos()
                ));
                assert!(payload.iter().flatten().all(|byte| *byte == 0x5a));
            }
        }
    }
    assert!(
        std::fs::read(&bulk.path)
            .unwrap()
            .iter()
            .all(|byte| *byte == 0x5a)
    );
    drop(bulk);
    rows.push(format!("\"small_files_512_ns\":{}", package_files(512, 8)));
    println!(
        "FILE_VECTOR_BENCH {{\"optimized\":{},{} }}",
        stream_optimized(),
        rows.join(",")
    );
}

fn package_files(count: usize, segments: usize) -> u128 {
    // A package-style complete operation chain, with all path construction and
    // final content verification outside the timer. Every timed file is new.
    let directory = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("kinakaze-file-vector-many-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let paths: Vec<_> = (0..count)
        .map(|index| directory.join(format!("package-{index}.json")))
        .collect();
    // Confine this last benchmark phase inside the executable's namespace
    // base so it exercises the production native exclusive-create/read opens.
    kinakaze_vfs::fs_context::unshare();
    let previous_root = kinakaze_vfs::path::system_root().unwrap();
    kinakaze_vfs::path::set_system_root(directory.clone());
    let names: Vec<_> = (0..count)
        .map(|index| format!("/package-{index}.json"))
        .collect();
    let length = 4096 / segments;
    let mut payload: Vec<_> = (0..segments).map(|_| vec![0x5au8; length + 64]).collect();
    let items: Vec<_> = payload
        .iter_mut()
        .map(|bytes| vector(&mut bytes[..length]))
        .collect();
    let began = std::time::Instant::now();
    for name in &names {
        let writer = fs::open(name, fs::O_WRONLY | fs::O_CREAT | fs::O_EXCL, 0o644).unwrap();
        assert_eq!(
            unsafe {
                crate::sysadmin::kinakaze_abi_syscall_raw(
                    if segments == 1 { 1 } else { 20 },
                    writer as u64,
                    if segments == 1 {
                        items[0].iov_base as u64
                    } else {
                        items.as_ptr() as u64
                    },
                    if segments == 1 { 4096 } else { segments as u64 },
                    0,
                    0,
                    0,
                )
            },
            4096
        );
        kinakaze_vfs::close(writer).unwrap();
        let reader = fs::open(name, fs::O_RDONLY, 0).unwrap();
        assert_eq!(
            unsafe {
                crate::sysadmin::kinakaze_abi_syscall_raw(
                    if segments == 1 { 0 } else { 19 },
                    reader as u64,
                    if segments == 1 {
                        items[0].iov_base as u64
                    } else {
                        items.as_ptr() as u64
                    },
                    if segments == 1 { 4096 } else { segments as u64 },
                    0,
                    0,
                    0,
                )
            },
            4096
        );
        kinakaze_vfs::close(reader).unwrap();
    }
    let elapsed = began.elapsed().as_nanos();
    for path in &paths {
        assert_eq!(std::fs::read(path).unwrap(), vec![0x5a; 4096]);
        std::fs::remove_file(path).unwrap();
    }
    kinakaze_vfs::path::set_system_root(previous_root);
    std::fs::remove_dir(directory).unwrap();
    elapsed
}

#[test]
#[ignore = "isolated complete small-file operation throughput comparison"]
fn benchmark_package_files() {
    let scalar = package_files(1024, 1);
    let vectored = package_files(1024, 8);
    println!(
        "SMALL_FILE_BENCH {{\"optimized\":{},\"scalar_files_1024_ns\":{scalar},\"vectored_files_1024_ns\":{vectored}}}",
        stream_optimized()
    );
}
