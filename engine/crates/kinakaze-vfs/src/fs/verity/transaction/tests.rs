use super::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("kinakaze-verity-tx-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn file(&self) -> PathBuf {
        let path = self.0.join("content");
        std::fs::write(&path, vec![0x5a; 20_073]).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Fd(i32);
impl Fd {
    fn read(path: &Path) -> Self {
        Self(crate::fs::open(&crate::path::to_guest_path(path), crate::fs::O_RDONLY, 0).unwrap())
    }
    fn opened(&self) -> Opened {
        Opened::from_fd(self.0).unwrap()
    }
    fn prepare(&self) -> Transaction {
        self.opened().prepare(1, 4096, b"transaction").unwrap()
    }
}
impl Drop for Fd {
    fn drop(&mut self) {
        crate::close(self.0).unwrap();
    }
}

fn assert_masked(handle: HANDLE) {
    assert_eq!(authoritative_size(handle).unwrap(), 20_073);
    let mut bytes = vec![0xcc; 30_000];
    assert_eq!(verified_read(handle, 0, &mut bytes), Ok(Some(20_073)));
    assert_eq!(&bytes[..20_073], vec![0x5a; 20_073]);
    assert!(bytes[20_073..].iter().all(|&byte| byte == 0xcc));
}

fn assert_writer_denied(path: &Path) {
    assert!(matches!(Object::open(path, GENERIC_WRITE), Err(EBUSY)));
}

#[test]
fn prepared_and_built_release_inode_mutex_but_keep_write_reservation() {
    let fixture = Fixture::new();
    let path = fixture.file();
    let fd = Fd::read(&path);
    let mut transaction = fd.prepare();
    for built in [false, true] {
        if built {
            transaction.build().unwrap();
        }
        assert_writer_denied(&path);
        let path = path.clone();
        let id = transaction.id();
        let (send, receive) = std::sync::mpsc::channel();
        let participant = std::thread::spawn(move || {
            let reader = Object::open(&path, GENERIC_READ | QUERY_ACCESS).unwrap();
            assert_masked(reader.raw());
            assert_eq!(ensure_writable(reader.raw()), Err(EBUSY));
            let expected = if built {
                Publication::Built(id)
            } else {
                Publication::Preparing(id)
            };
            assert_eq!(publication(reader.raw()), Ok(expected));
            if built {
                assert!(staged_descriptor(reader.raw(), id).is_ok());
            } else {
                assert!(matches!(staged_descriptor(reader.raw(), id), Err(EBUSY)));
            }
            send.send(()).unwrap();
        });
        let result = receive.recv_timeout(Duration::from_secs(3));
        // If a regression retained the mutex, Drop releases it before join.
        if result.is_err() {
            drop(transaction);
            participant.join().unwrap();
            panic!("participant blocked behind transaction owner");
        }
        participant.join().unwrap();
        assert_eq!(fd.opened().prepare(1, 4096, b"other").unwrap_err(), EBUSY);
    }
    transaction.commit().unwrap();
    assert_eq!(transaction.state(), TransactionState::Committed);
    assert_writer_denied(&path);
    assert_eq!(transaction.abort(), Err(EPERM));
    drop(transaction);
    let host_writer = Object::open(&path, GENERIC_WRITE).unwrap();
    assert_eq!(ensure_writable(host_writer.raw()), Err(EPERM));
}

#[test]
fn stale_token_cannot_read_a_later_transaction_or_an_aborted_tree() {
    let fixture = Fixture::new();
    let path = fixture.file();
    let fd = Fd::read(&path);
    let reader = fd.opened();
    let mut old = fd.prepare();
    old.build().unwrap();
    let token = old.id();
    old.abort().unwrap();
    assert!(matches!(
        staged_descriptor(reader.borrowed_handle(), token),
        Err(116)
    ));
    drop(old);
    let mut current = fd.prepare();
    assert_ne!(current.id(), token);
    current.build().unwrap();
    let mut bytes = [0xcc; 20];
    assert_eq!(
        staged_verified_read(reader.borrowed_handle(), token, 0, &mut bytes),
        Err(116)
    );
    assert_eq!(bytes, [0xcc; 20]);
    assert_eq!(
        staged_verified_read(reader.borrowed_handle(), current.id(), 0, &mut bytes),
        Ok(20)
    );
    assert_eq!(bytes, [0x5a; 20]);
    assert_eq!(reader.measure(), Err(ENODATA));
    current.commit().unwrap();
    assert_eq!(
        staged_descriptor(reader.borrowed_handle(), current.id()).unwrap(),
        *current.descriptor().unwrap()
    );
}

#[test]
fn transaction_pin_survives_fd_close_unlink_and_path_replacement() {
    let fixture = Fixture::new();
    let path = fixture.file();
    let fd = Fd::read(&path);
    let mut transaction = fd.prepare();
    let id = transaction.id();
    drop(fd);
    crate::fs::unlink(&crate::path::to_guest_path(&path)).unwrap();
    std::fs::write(&path, b"a different inode").unwrap();
    transaction.build().unwrap();
    let mut bytes = [0xcc; 20];
    assert_eq!(
        staged_verified_read(transaction.borrowed_handle(), id, 0, &mut bytes),
        Ok(20)
    );
    assert_eq!(bytes, [0x5a; 20]);
    transaction.commit().unwrap();
    assert_eq!(transaction.id(), id);
    assert_eq!(std::fs::read(&path).unwrap(), b"a different inode");
    let replacement = Fd::read(&path);
    assert_eq!(replacement.opened().measure(), Err(ENODATA));
}

#[test]
fn abandoned_prepared_and_built_are_recovered_by_real_writer_handles() {
    for built in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.file();
        let fd = Fd::read(&path);
        let mut transaction = fd.prepare();
        if built {
            transaction.build().unwrap();
        }
        drop(transaction); // No Drop I/O: record must still be present.
        let writer = Object::open(&path, GENERIC_READ | GENERIC_WRITE | QUERY_ACCESS).unwrap();
        assert_ne!(publication(writer.raw()).unwrap(), Publication::Absent);
        assert_masked(writer.raw());
        ensure_writable(writer.raw()).unwrap();
        assert_eq!(publication(writer.raw()).unwrap(), Publication::Absent);
        assert_eq!(native_size(&writer).unwrap(), 20_073);
        assert_eq!(std::fs::read(&path).unwrap(), vec![0x5a; 20_073]);
    }
}

#[test]
fn build_errors_keep_lease_and_hidden_tail_until_explicit_abort() {
    for error in [EIO, crate::EINTR] {
        for point in [
            Checkpoint::Read,
            Checkpoint::Write,
            Checkpoint::TreeFlush,
            Checkpoint::BuiltWrite,
            Checkpoint::BuiltFlush,
        ] {
            let fixture = Fixture::new();
            let path = fixture.file();
            let fd = Fd::read(&path);
            let mut transaction = fd.prepare();
            transaction.fault.set(Some((point, error)));
            assert_eq!(transaction.build(), Err(error), "{point:?}");
            assert_writer_denied(&path);
            assert!(descriptor(transaction.borrowed_handle()).unwrap().is_none());
            assert_masked(transaction.borrowed_handle());
            assert_eq!(transaction.commit(), Err(EINVAL));
            transaction.abort().unwrap();
            assert_eq!(transaction.state(), TransactionState::Aborted);
            assert_eq!(native_size(&transaction.writer).unwrap(), 20_073);
            assert_eq!(
                publication(transaction.borrowed_handle()),
                Ok(Publication::Absent)
            );
            assert_writer_denied(&path);
        }
    }
}

#[test]
fn commit_errors_distinguish_unpublished_from_enabled_without_durability_claims() {
    for point in [
        Checkpoint::CommitWrite,
        Checkpoint::CommitAfterWrite,
        Checkpoint::CommitFlush,
    ] {
        for error in [EIO, crate::EINTR] {
            let fixture = Fixture::new();
            let path = fixture.file();
            let fd = Fd::read(&path);
            let mut transaction = fd.prepare();
            transaction.build().unwrap();
            transaction.fault.set(Some((point, error)));
            assert_eq!(transaction.commit(), Err(error));
            assert_writer_denied(&path);
            if point == Checkpoint::CommitWrite {
                assert_eq!(transaction.state(), TransactionState::Built);
                assert_eq!(
                    publication(transaction.borrowed_handle()),
                    Ok(Publication::Built(transaction.id()))
                );
                assert_eq!(fd.opened().measure(), Err(ENODATA));
                transaction.abort().unwrap();
            } else {
                assert_eq!(transaction.state(), TransactionState::Published);
                assert_eq!(
                    publication(transaction.borrowed_handle()),
                    Ok(Publication::Enabled(transaction.id()))
                );
                assert_eq!(transaction.abort(), Err(EPERM));
                assert!(fd.opened().measure().is_ok());
                assert_masked(transaction.borrowed_handle());
                transaction.commit().unwrap();
                assert_eq!(transaction.state(), TransactionState::Committed);
            }
        }
    }
}

#[test]
fn failed_rollback_never_clears_marker_before_truncation_is_flushed() {
    for point in [
        Checkpoint::RollbackTruncate,
        Checkpoint::RollbackFlush,
        Checkpoint::RollbackDelete,
        Checkpoint::RollbackFinalFlush,
    ] {
        let fixture = Fixture::new();
        let path = fixture.file();
        let fd = Fd::read(&path);
        let mut transaction = fd.prepare();
        transaction.build().unwrap();
        transaction.fault.set(Some((point, EIO)));
        assert_eq!(transaction.abort(), Err(EIO));
        assert_eq!(transaction.state(), TransactionState::Indeterminate);
        assert_writer_denied(&path);
        assert_masked(transaction.borrowed_handle());
        if point == Checkpoint::RollbackFinalFlush {
            assert_eq!(
                publication(transaction.borrowed_handle()),
                Ok(Publication::Absent)
            );
            assert_eq!(native_size(&transaction.writer), Ok(20_073));
        } else {
            assert_eq!(
                publication(transaction.borrowed_handle()),
                Ok(Publication::Built(transaction.id()))
            );
        }
        transaction.abort().unwrap();
        assert_eq!(transaction.state(), TransactionState::Aborted);
        assert_eq!(
            publication(transaction.borrowed_handle()),
            Ok(Publication::Absent)
        );
    }
}

#[test]
fn v2_format_rejects_legacy_or_missing_transaction_identity() {
    let descriptor = Descriptor::new(1, 4096, b"", 100).unwrap();
    let record = Record::new(PREPARING, descriptor).unwrap();
    let good = record.encode();
    assert_eq!(good.len(), 320);
    assert_eq!(
        Record::decode(&good).unwrap().transaction,
        record.transaction
    );
    let mut legacy = [0u8; 288];
    legacy[..8].copy_from_slice(b"CYVERIT1");
    assert!(matches!(Record::decode(&legacy), Err(EIO)));
    for range in [0..8, 32..48] {
        let mut bad = good;
        bad[range].fill(0);
        assert!(matches!(Record::decode(&bad), Err(EIO)));
    }
}

#[test]
fn resume_keeps_identity_and_never_guesses_enabled_flush_completion() {
    for phase in [
        TransactionState::Prepared,
        TransactionState::Built,
        TransactionState::Published,
    ] {
        let fixture = Fixture::new();
        let path = fixture.file();
        let fd = Fd::read(&path);
        let mut transaction = fd.prepare();
        let id = transaction.id();
        if phase != TransactionState::Prepared {
            transaction.build().unwrap();
        }
        if phase == TransactionState::Published {
            transaction.fault.set(Some((Checkpoint::CommitFlush, EIO)));
            assert_eq!(transaction.commit(), Err(EIO));
        }
        assert_eq!(fd.opened().resume(id).unwrap_err(), EBUSY);
        drop(transaction);
        assert_eq!(
            fd.opened()
                .resume(TransactionId::new().unwrap())
                .unwrap_err(),
            116
        );
        let mut resumed = fd.opened().resume(id).unwrap();
        assert_eq!(resumed.id(), id);
        assert_eq!(resumed.state(), phase);
        assert_writer_denied(&path);
        if phase == TransactionState::Prepared {
            resumed.build().unwrap();
        }
        if phase == TransactionState::Published {
            assert_eq!(resumed.abort(), Err(EPERM));
        }
        resumed.commit().unwrap();
        assert_eq!(resumed.state(), TransactionState::Committed);
        assert_masked(resumed.borrowed_handle());
    }
}

#[test]
fn staged_corruption_never_reaches_participants_or_publishes_a_different_root() {
    for corrupt_tree in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.file();
        let fd = Fd::read(&path);
        let mut transaction = fd.prepare();
        transaction.build().unwrap();
        // Deliberate host-side corruption through the owner, not guest writes.
        let offset = if corrupt_tree {
            transaction.record.tree_offset
        } else {
            1
        };
        exact_write(&transaction.writer, offset, &[0xff]).unwrap();
        transaction.writer.flush().unwrap();
        let mut bytes = [0xcc; 20];
        assert_eq!(
            staged_verified_read(
                transaction.borrowed_handle(),
                transaction.id(),
                0,
                &mut bytes
            ),
            Err(EIO)
        );
        assert_eq!(bytes, [0xcc; 20]);
        transaction.abort().unwrap();
    }
}

#[test]
fn indeterminate_built_and_published_can_be_reconciled_without_releasing_reservation() {
    for published in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.file();
        let fd = Fd::read(&path);
        let mut transaction = fd.prepare();
        if published {
            transaction.build().unwrap();
            transaction.fault.set(Some((Checkpoint::CommitFlush, EIO)));
            assert_eq!(transaction.commit(), Err(EIO));
        } else {
            transaction.fault.set(Some((Checkpoint::BuiltFlush, EIO)));
            assert_eq!(transaction.build(), Err(EIO));
            assert_eq!(transaction.state(), TransactionState::Indeterminate);
        }
        assert_eq!(
            transaction.reconcile(),
            Ok(if published {
                TransactionState::Published
            } else {
                TransactionState::Built
            })
        );
        assert_writer_denied(&path);
        transaction.commit().unwrap();
        assert_eq!(transaction.state(), TransactionState::Committed);
    }
}

#[test]
fn partially_rolled_back_built_record_cannot_be_reconciled_or_resumed_as_committable() {
    let fixture = Fixture::new();
    let path = fixture.file();
    let fd = Fd::read(&path);
    let mut transaction = fd.prepare();
    transaction.build().unwrap();
    let id = transaction.id();
    transaction
        .fault
        .set(Some((Checkpoint::RollbackDelete, EIO)));
    assert_eq!(transaction.abort(), Err(EIO));
    assert_eq!(
        publication(transaction.borrowed_handle()),
        Ok(Publication::Built(id))
    );
    assert_eq!(transaction.reconcile(), Err(EIO));
    assert_eq!(transaction.state(), TransactionState::Indeterminate);
    assert_eq!(transaction.commit(), Err(EINVAL));
    drop(transaction);
    let mut resumed = fd.opened().resume(id).unwrap();
    assert_eq!(resumed.state(), TransactionState::Indeterminate);
    assert_eq!(resumed.commit(), Err(EINVAL));
    resumed.abort().unwrap();
    assert_eq!(
        publication(resumed.borrowed_handle()),
        Ok(Publication::Absent)
    );
}

#[test]
fn abort_keeps_marker_until_native_readonly_view_is_retired_before_eof_shrink() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_READ, MapViewOfFile, PAGE_NOACCESS, PAGE_READONLY,
        UnmapViewOfFile, VirtualProtect,
    };
    let fixture = Fixture::new();
    let path = fixture.file();
    std::fs::write(&path, vec![0x5a; 17_003]).unwrap();
    let fd = Fd::read(&path);
    let opened = fd.opened();
    struct Mapping(
        HANDLE,
        windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS,
    );
    impl Drop for Mapping {
        fn drop(&mut self) {
            unsafe {
                UnmapViewOfFile(self.1);
                CloseHandle(self.0);
            }
        }
    }
    let map = || {
        let section = unsafe {
            CreateFileMappingW(
                opened.borrowed_handle(),
                std::ptr::null(),
                PAGE_READONLY,
                0,
                0,
                std::ptr::null(),
            )
        };
        assert!(!section.is_null());
        let view = unsafe { MapViewOfFile(section, FILE_MAP_READ, 0, 0, 0) };
        assert!(!view.Value.is_null());
        Mapping(section, view)
    };
    let mapping = map();
    let mut old = 0;
    assert_ne!(
        unsafe { VirtualProtect(mapping.1.Value, 20_480, PAGE_NOACCESS, &mut old) },
        0
    );
    let mut transaction = opened.prepare(1, 1024, b"parked").unwrap();
    // PAGE_NOACCESS is not native view retirement. Windows permits append but
    // refuses this shrink while the old EOF-padding page remains mapped.
    assert!(transaction.record.tree_offset < 20_480);
    transaction.build().unwrap();
    assert_eq!(transaction.abort(), Err(EIO));
    assert_eq!(transaction.state(), TransactionState::Indeterminate);
    assert_eq!(
        publication(transaction.borrowed_handle()),
        Ok(Publication::Built(transaction.id()))
    );
    assert_eq!(
        authoritative_size(transaction.borrowed_handle()),
        Ok(17_003)
    );
    assert_writer_denied(&path);
    drop(mapping);
    transaction.abort().unwrap();
    assert_eq!(transaction.state(), TransactionState::Aborted);
    assert_eq!(
        publication(transaction.borrowed_handle()),
        Ok(Publication::Absent)
    );
    let restored = map();
    let bytes = unsafe { std::slice::from_raw_parts(restored.1.Value.cast::<u8>(), 20_480) };
    assert!(bytes[..17_003].iter().all(|&byte| byte == 0x5a));
    assert!(bytes[17_003..].iter().all(|&byte| byte == 0));
}

fn child(path: &Path, scenario: &str, token: Option<TransactionId>) -> std::process::Output {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    let test = format!(
        "{}::transaction_process_helper",
        module_path!().split_once("::").unwrap().1
    );
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", &test, "--nocapture"])
        .env("KINAKAZE_VERITY_TRANSACTION_PATH", path)
        .env("KINAKAZE_VERITY_TRANSACTION_SCENARIO", scenario)
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(token) = token {
        command.env(
            "KINAKAZE_VERITY_TRANSACTION_ID",
            format!("{:032x}", u128::from_le_bytes(*token.as_bytes())),
        );
    }
    let mut child = crate::with_exec_handle_filter(|| command.spawn())
        .unwrap()
        .unwrap();
    let waited = unsafe { WaitForSingleObject(child.as_raw_handle() as HANDLE, 10_000) };
    if waited != WAIT_OBJECT_0 {
        child.kill().unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        waited,
        WAIT_OBJECT_0,
        "child timed out: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn transaction_process_helper() {
    let Some(path) = std::env::var_os("KINAKAZE_VERITY_TRANSACTION_PATH") else {
        return;
    };
    let path = Path::new(&path);
    let scenario = std::env::var("KINAKAZE_VERITY_TRANSACTION_SCENARIO").unwrap();
    let fd = Fd::read(path);
    if scenario == "peer" {
        let bytes = u128::from_str_radix(
            &std::env::var("KINAKAZE_VERITY_TRANSACTION_ID").unwrap(),
            16,
        )
        .unwrap()
        .to_le_bytes();
        let id = TransactionId::from_bytes(bytes).unwrap();
        let opened = fd.opened();
        assert_eq!(
            publication(opened.borrowed_handle()),
            Ok(Publication::Built(id))
        );
        assert!(opened.descriptor().unwrap().is_none());
        assert_eq!(ensure_writable(opened.borrowed_handle()), Err(EBUSY));
        assert_writer_denied(path);
        let mut bytes = vec![0xcc; 30_000];
        assert_eq!(
            staged_verified_read(opened.borrowed_handle(), id, 0, &mut bytes),
            Ok(20_073)
        );
        assert_eq!(&bytes[..20_073], vec![0x5a; 20_073]);
        assert!(bytes[20_073..].iter().all(|&byte| byte == 0xcc));
        return;
    }
    let mut transaction = fd.prepare();
    if matches!(scenario.as_str(), "fatal" | "fatal_commit") {
        if scenario == "fatal_commit" {
            transaction.build().unwrap();
        }
        crate::signal::raise_thread_signal(
            crate::interrupt::current_thread_id(),
            crate::signal::SIGTERM,
        )
        .unwrap();
        assert!(crate::signal::fatal_pending().unwrap());
        if scenario == "fatal_commit" {
            assert_eq!(transaction.commit(), Err(crate::EINTR));
            assert_eq!(transaction.state(), TransactionState::Built);
            assert_eq!(
                publication(transaction.borrowed_handle()),
                Ok(Publication::Built(transaction.id()))
            );
        } else {
            assert_eq!(transaction.build(), Err(crate::EINTR));
        }
        assert!(
            crate::signal::fatal_pending().unwrap(),
            "no handler consumed the signal"
        );
        crate::signal::swap_blocked_mask(1 << (crate::signal::SIGTERM - 1));
        transaction.abort().unwrap();
        assert_eq!(
            publication(transaction.borrowed_handle()),
            Ok(Publication::Absent)
        );
        return;
    }
    if scenario != "prepared" {
        transaction.build().unwrap();
    }
    if scenario == "committed" {
        transaction.commit().unwrap();
    }
    if scenario == "published" {
        transaction.fault.set(Some((Checkpoint::CommitFlush, EIO)));
        assert_eq!(transaction.commit(), Err(EIO));
        assert_eq!(transaction.state(), TransactionState::Published);
    }
    // Real process death: no Rust destructors or implicit cleanup.
    std::process::exit(37);
}

#[test]
fn staged_verification_works_in_an_independent_native_process() {
    let fixture = Fixture::new();
    let path = fixture.file();
    let fd = Fd::read(&path);
    let mut transaction = fd.prepare();
    transaction.build().unwrap();
    let output = child(&path, "peer", Some(transaction.id()));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        publication(transaction.borrowed_handle()),
        Ok(Publication::Built(transaction.id()))
    );
    transaction.commit().unwrap();
}

#[test]
fn native_process_death_recovers_prepared_built_and_preserves_committed() {
    for phase in ["prepared", "built", "committed"] {
        let fixture = Fixture::new();
        let path = fixture.file();
        let output = child(&path, phase, None);
        assert_eq!(
            output.status.code(),
            Some(37),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let fd = Fd::read(&path);
        let opened = fd.opened();
        assert_masked(opened.borrowed_handle());
        if phase == "committed" {
            assert!(matches!(
                publication(opened.borrowed_handle()),
                Ok(Publication::Enabled(_))
            ));
            assert_eq!(ensure_writable(opened.borrowed_handle()), Err(EPERM));
        } else {
            assert!(matches!(
                publication(opened.borrowed_handle()),
                Ok(Publication::Preparing(_) | Publication::Built(_))
            ));
            let mut recovered = fd.prepare();
            assert_eq!(native_size(&recovered.writer), Ok(20_073));
            recovered.build().unwrap();
            recovered.commit().unwrap();
        }
    }
}

#[test]
fn fatal_signal_unwinds_transaction_before_any_guest_handler() {
    for scenario in ["fatal", "fatal_commit"] {
        let fixture = Fixture::new();
        let path = fixture.file();
        let output = child(&path, scenario, None);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn abrupt_native_coordinator_exit_can_be_resumed_without_guessing_or_losing_identity() {
    for phase in ["prepared", "built", "published", "committed"] {
        for abort in [false, true] {
            let fixture = Fixture::new();
            let path = fixture.file();
            let output = child(&path, phase, None);
            assert_eq!(
                output.status.code(),
                Some(37),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let fd = Fd::read(&path);
            let opened = fd.opened();
            let state = publication(opened.borrowed_handle()).unwrap();
            let id = match state {
                Publication::Preparing(id) | Publication::Built(id) | Publication::Enabled(id) => {
                    id
                }
                Publication::Absent => panic!("abrupt child must leave its durable identity"),
            };
            let mut transaction = opened.resume(id).unwrap();
            assert_eq!(transaction.id(), id);
            assert_writer_denied(&path);
            if matches!(state, Publication::Enabled(_)) {
                assert_eq!(transaction.state(), TransactionState::Published);
                assert_eq!(transaction.abort(), Err(EPERM));
                transaction.commit().unwrap();
                assert_eq!(transaction.state(), TransactionState::Committed);
            } else if abort {
                transaction.abort().unwrap();
                assert_eq!(
                    publication(opened.borrowed_handle()),
                    Ok(Publication::Absent)
                );
                assert_eq!(native_size(&transaction.writer), Ok(20_073));
            } else {
                if transaction.state() == TransactionState::Prepared {
                    transaction.build().unwrap();
                }
                transaction.commit().unwrap();
                assert_eq!(
                    publication(opened.borrowed_handle()),
                    Ok(Publication::Enabled(id))
                );
            }
            assert_masked(opened.borrowed_handle());
        }
    }
}
