use super::*;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::Command;

fn mapping(name: &str) -> (Handle, MEMORY_MAPPED_VIEW_ADDRESS) {
    let section = Handle::new(unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            0,
            4096,
            wide(name).as_ptr(),
        )
    })
    .unwrap();
    let view = unsafe { MapViewOfFile(section.0, FILE_MAP_ALL_ACCESS, 0, 0, 4096) };
    assert!(!view.Value.is_null());
    (section, view)
}
#[test]
#[ignore = "real process helper for requeue journal crash recovery"]
fn proxy_child() {
    if let Ok(domain) = std::env::var("KINAKAZE_REQUEUE_DOMAIN") {
        kinakaze_runtime::authority::install_helper_domain(domain.parse().unwrap()).unwrap();
    }
    let name = std::env::var("KINAKAZE_REQUEUE_MAPPING").unwrap();
    let (_section, original) = mapping(&name);
    let (_alias_section, alias) = mapping(&name);
    let parent_address = std::env::var("KINAKAZE_REQUEUE_PARENT_ADDRESS")
        .ok()
        .map(|value| value.parse::<usize>().unwrap());
    let view = if parent_address == Some(original.Value as usize) {
        alias
    } else {
        original
    };
    if let Some(parent_address) = parent_address {
        assert_ne!(view.Value as usize, parent_address);
    }
    let values: Vec<u64> = std::env::var("KINAKAZE_REQUEUE_BACKING")
        .unwrap()
        .split(',')
        .map(|part| part.parse().unwrap())
        .collect();
    let backing: [u64; 3] = values.try_into().unwrap();
    let source = Key::anonymous(backing, 0);
    let target = Key::anonymous(backing, 4);
    let mode = std::env::var("KINAKAZE_REQUEUE_MODE").unwrap_or_default();
    if mode == "source-hold" {
        let tid = crate::fsextra::kinakaze_abi_gettid() as u32;
        let mut transaction = Transaction::begin().unwrap();
        enqueue(
            &mut transaction,
            source,
            target,
            view.Value as usize,
            0,
            tid,
        )
        .unwrap();
        drop(transaction);
        let ready_name = std::env::var("KINAKAZE_REQUEUE_READY").unwrap();
        let ready =
            Handle::new(unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, wide(&ready_name).as_ptr()) })
                .unwrap();
        assert_ne!(unsafe { SetEvent(ready.0) }, 0);
        unsafe { WaitForSingleObject(park().unwrap(), INFINITE) };
        panic!("parent did not terminate the source holder");
    }
    if mode.starts_with("readonly") {
        let mut old = 0;
        assert_ne!(
            unsafe {
                windows_sys::Win32::System::Memory::VirtualProtect(
                    view.Value,
                    4096,
                    windows_sys::Win32::System::Memory::PAGE_READONLY,
                    &mut old,
                )
            },
            0
        );
    }
    let mut transaction = Transaction::begin().unwrap();
    if mode == "readonly-error" {
        assert_eq!(
            transfer(
                &mut transaction,
                source,
                target,
                view.Value as usize,
                view.Value as usize + 4,
                0,
                1
            ),
            Err(kinakaze_vfs::EFAULT)
        );
        drop(transaction);
        unsafe {
            UnmapViewOfFile(original);
            UnmapViewOfFile(alias);
        }
        return;
    }
    assert_eq!(
        transfer(
            &mut transaction,
            source,
            target,
            view.Value as usize,
            view.Value as usize + 4,
            0,
            1
        ),
        Ok(1)
    );
    panic!("proxy crash hook did not execute");
}

#[test]
fn requeue_proxy_crash_recovers_without_false_owner_death() {
    for phase in [
        "before-journal",
        "before-cas",
        "after-cas",
        "after-publication",
    ] {
        let backing = new_backing_id().unwrap();
        let source = Key::anonymous(backing, 0);
        let target = Key::anonymous(backing, 4);
        let name = format!(
            r"Local\kinakaze.requeue.test.{}.{}.{}",
            backing[0], backing[1], backing[2]
        );
        let (_section, view) = mapping(&name);
        let tid = crate::fsextra::kinakaze_abi_gettid() as u32;
        let park = park().unwrap();
        unsafe { windows_sys::Win32::System::Threading::ResetEvent(park) };
        let mut transaction = Transaction::begin().unwrap();
        let token = enqueue(
            &mut transaction,
            source,
            target,
            view.Value as usize,
            0,
            tid,
        )
        .unwrap();
        drop(transaction);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "futex::pi::requeue::tests::proxy_child",
                "--ignored",
                "--nocapture",
            ])
            .env("KINAKAZE_PI_CRASH", phase)
            .env("KINAKAZE_REQUEUE_MAPPING", &name)
            .env(
                "KINAKAZE_REQUEUE_BACKING",
                backing.map(|part| part.to_string()).join(","),
            )
            .creation_flags(0x0800_0000)
            .spawn()
            .unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(park, 5000) },
            WAIT_OBJECT_0,
            "{phase}: source waiter was not notified"
        );
        assert_eq!(child.wait().unwrap().code(), Some(77));
        let mut transaction = Transaction::begin().unwrap();
        if phase == "before-journal" {
            assert!(matches!(
                poll_waiter(
                    &mut transaction,
                    target,
                    view.Value as usize + 4,
                    token,
                    false,
                    true
                )
                .unwrap(),
                Progress::Source
            ));
            assert_eq!(atomic_word::read(view.Value as usize + 4), Ok(0));
            assert_eq!(
                transfer(
                    &mut transaction,
                    source,
                    target,
                    view.Value as usize,
                    view.Value as usize + 4,
                    0,
                    1
                ),
                Ok(1)
            );
        }
        assert!(matches!(
            poll_waiter(
                &mut transaction,
                target,
                view.Value as usize + 4,
                token,
                false,
                true
            )
            .unwrap(),
            Progress::Owned
        ));
        assert_eq!(
            atomic_word::read(view.Value as usize + 4).unwrap() & (OWNER_DIED | TID_MASK),
            tid,
            "{phase}"
        );
        unlock(&mut transaction, target, view.Value as usize + 4, tid).unwrap();
        assert!(
            !transaction
                .records
                .iter()
                .any(|row| row.key == source || row.key == target)
        );
        drop(transaction);
        unsafe { UnmapViewOfFile(view) };
    }
}

#[test]
#[ignore = "paired proxy requeue ownership microbenchmark"]
fn benchmark_requeue_proxy() {
    let source = AtomicU32::new(0);
    let target = AtomicU32::new(0);
    let source_key = Key::flagged_private(source.as_ptr() as usize | 1).unwrap();
    let target_key = Key::flagged_private(target.as_ptr() as usize | 1).unwrap();
    let tid = crate::fsextra::kinakaze_abi_gettid() as u32;
    let iterations = 1000;
    let started = Instant::now();
    for _ in 0..iterations {
        let mut transaction = Transaction::begin().unwrap();
        let token = enqueue(
            &mut transaction,
            source_key,
            target_key,
            source.as_ptr() as usize,
            0,
            tid,
        )
        .unwrap();
        assert_eq!(
            transfer(
                &mut transaction,
                source_key,
                target_key,
                source.as_ptr() as usize,
                target.as_ptr() as usize,
                0,
                1
            ),
            Ok(1)
        );
        assert!(matches!(
            poll_waiter(
                &mut transaction,
                target_key,
                target.as_ptr() as usize,
                token,
                false,
                true
            )
            .unwrap(),
            Progress::Owned
        ));
        unlock(&mut transaction, target_key, target.as_ptr() as usize, tid).unwrap();
    }
    println!(
        "REQUEUE_PI_BENCH iterations={iterations} elapsed_ns={} optimized={}",
        started.elapsed().as_nanos(),
        optimized()
    );
}

#[test]
fn requeue_proxy_readonly_alias_preserves_source_or_recovers_through_live_alias() {
    for crash in [false, true] {
        let backing = new_backing_id().unwrap();
        let source = Key::anonymous(backing, 0);
        let target = Key::anonymous(backing, 4);
        let name = format!(
            r"Local\kinakaze.requeue.readonly.{}.{}.{}",
            backing[0], backing[1], backing[2]
        );
        let (_section, view) = mapping(&name);
        let tid = crate::fsextra::kinakaze_abi_gettid() as u32;
        let park = park().unwrap();
        unsafe { windows_sys::Win32::System::Threading::ResetEvent(park) };
        let mut transaction = Transaction::begin().unwrap();
        let token = enqueue(
            &mut transaction,
            source,
            target,
            view.Value as usize,
            0,
            tid,
        )
        .unwrap();
        drop(transaction);
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "futex::pi::requeue::tests::proxy_child",
                "--ignored",
                "--nocapture",
            ])
            .env("KINAKAZE_REQUEUE_MAPPING", &name)
            .env(
                "KINAKAZE_REQUEUE_BACKING",
                backing.map(|part| part.to_string()).join(","),
            )
            .env(
                "KINAKAZE_REQUEUE_PARENT_ADDRESS",
                (view.Value as usize).to_string(),
            )
            .env(
                "KINAKAZE_REQUEUE_MODE",
                if crash {
                    "readonly-crash"
                } else {
                    "readonly-error"
                },
            )
            .creation_flags(0x0800_0000);
        if crash {
            command.env("KINAKAZE_PI_CRASH", "before-cas");
        } else {
            command.env_remove("KINAKAZE_PI_CRASH");
        }
        let mut child = command.spawn().unwrap();
        assert_eq!(unsafe { WaitForSingleObject(park, 5000) }, WAIT_OBJECT_0);
        assert_eq!(
            child.wait().unwrap().code(),
            Some(if crash { 77 } else { 0 })
        );
        let mut transaction = Transaction::begin().unwrap();
        if !crash {
            assert_eq!(atomic_word::read(view.Value as usize + 4), Ok(0));
            assert!(matches!(
                poll_waiter(
                    &mut transaction,
                    target,
                    view.Value as usize + 4,
                    token,
                    false,
                    true
                )
                .unwrap(),
                Progress::Source
            ));
            assert_eq!(
                transfer(
                    &mut transaction,
                    source,
                    target,
                    view.Value as usize,
                    view.Value as usize + 4,
                    0,
                    1
                ),
                Ok(1)
            );
        }
        assert!(matches!(
            poll_waiter(
                &mut transaction,
                target,
                view.Value as usize + 4,
                token,
                false,
                true
            )
            .unwrap(),
            Progress::Owned
        ));
        assert_eq!(
            atomic_word::read(view.Value as usize + 4).unwrap() & (TID_MASK | OWNER_DIED),
            tid
        );
        unlock(&mut transaction, target, view.Value as usize + 4, tid).unwrap();
        assert!(
            !transaction
                .records
                .iter()
                .any(|record| record.key == source || record.key == target)
        );
        drop(transaction);
        unsafe {
            UnmapViewOfFile(view);
        }
    }
}

#[test]
#[ignore = "isolated shared-bank capacity and dead-process source reclamation helper"]
fn capacity_child() {
    let domain = process_birth().unwrap();
    kinakaze_runtime::authority::install_helper_domain(domain).unwrap();
    let source_word = AtomicU32::new(0);
    let target_word = AtomicU32::new(0);
    let source = Key::flagged_private(source_word.as_ptr() as usize).unwrap();
    let target = Key::flagged_private(target_word.as_ptr() as usize).unwrap();
    let tid = crate::fsextra::kinakaze_abi_gettid() as u32;
    let mut transaction = Transaction::begin().unwrap();
    let token = enqueue(
        &mut transaction,
        source,
        target,
        source_word.as_ptr() as usize,
        0,
        tid,
    )
    .unwrap();
    let before = transaction.records.len();
    assert_eq!(transaction.reserve(CAPACITY - before + 1), Err(ENOMEM));
    assert_eq!(transaction.records.len(), before);
    assert!(matches!(
        poll_waiter(
            &mut transaction,
            target,
            target_word.as_ptr() as usize,
            token,
            false,
            true
        )
        .unwrap(),
        Progress::Source
    ));
    retire_requeue(&mut transaction, target, token);
    assert!(transaction.records.is_empty());
    drop(transaction);
    let backing = new_backing_id().unwrap();
    let source = Key::anonymous(backing, 0);
    let target = Key::anonymous(backing, 4);
    let name = format!(
        r"Local\kinakaze.requeue.capacity.{}.{}.{}",
        backing[0], backing[1], backing[2]
    );
    let (_section, view) = mapping(&name);
    let ready_name = format!("{name}.ready");
    let ready = Handle::new(unsafe { CreateEventW(ptr::null(), 1, 0, wide(&ready_name).as_ptr()) })
        .unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "futex::pi::requeue::tests::proxy_child",
            "--ignored",
            "--nocapture",
        ])
        .env("KINAKAZE_REQUEUE_DOMAIN", domain.to_string())
        .env("KINAKAZE_REQUEUE_MAPPING", &name)
        .env(
            "KINAKAZE_REQUEUE_BACKING",
            backing.map(|part| part.to_string()).join(","),
        )
        .env("KINAKAZE_REQUEUE_MODE", "source-hold")
        .env("KINAKAZE_REQUEUE_READY", &ready_name)
        .env_remove("KINAKAZE_PI_CRASH")
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap();
    assert_eq!(unsafe { WaitForSingleObject(ready.0, 5000) }, WAIT_OBJECT_0);
    let mut transaction = Transaction::begin().unwrap();
    let before = transaction.records.len();
    assert_eq!(before, 2);
    assert_ne!(
        unsafe {
            windows_sys::Win32::System::Threading::TerminateProcess(child.as_raw_handle(), 77)
        },
        0
    );
    assert_eq!(child.wait().unwrap().code(), Some(77));
    assert!(
        transaction
            .records
            .iter()
            .any(|record| record.reserved & TARGET != 0 && record.key == target)
    );
    assert!(
        transaction
            .records
            .iter()
            .any(|record| record.pi_source() && record.key == source && record.dead())
    );
    transaction.reserve(CAPACITY - before + 1).unwrap();
    assert!(transaction.records.is_empty());
    transaction.commit();
    drop(transaction);
    unsafe {
        UnmapViewOfFile(view);
    }
}

#[test]
fn requeue_bank_exhaustion_keeps_live_source_and_reclaims_dead_binding() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "futex::pi::requeue::tests::capacity_child",
            "--ignored",
            "--nocapture",
        ])
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "capacity helper failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
