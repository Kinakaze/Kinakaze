use super::*;

pub(super) fn crash_at(phase: &str) {
    if std::env::var("KINAKAZE_PI_CRASH").is_ok_and(|value| value == phase) {
        unsafe { windows_sys::Win32::System::Threading::ExitProcess(77) };
    }
}

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
#[ignore = "subprocess helper for PI handoff crash recovery"]
fn handoff_child() {
    let name = std::env::var("KINAKAZE_PI_MAPPING").unwrap();
    let values: Vec<u64> = std::env::var("KINAKAZE_PI_KEY")
        .unwrap()
        .split(',')
        .map(|part| part.parse().unwrap())
        .collect();
    let key = Key(values.try_into().unwrap());
    let (_section, view) = mapping(&name);
    let ready = Handle::new(unsafe {
        OpenEventW(
            EVENT_MODIFY_STATE,
            0,
            wide(&format!("{name}.ready")).as_ptr(),
        )
    })
    .unwrap();
    let tid = crate::fsextra::kinakaze_abi_gettid() as u32;
    let mut transaction = Transaction::begin().unwrap();
    assert!(matches!(
        acquire(&mut transaction, key, view.Value as usize, tid, false),
        Ok(Acquisition::Owned)
    ));
    drop(transaction);
    assert_ne!(unsafe { SetEvent(ready.0) }, 0);
    let started = Instant::now();
    while count_for_test(key) == 0 {
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::yield_now();
    }
    let mut transaction = Transaction::begin().unwrap();
    unlock(&mut transaction, key, view.Value as usize, tid).unwrap();
    panic!("crash phase did not run");
}

#[test]
fn pi_cross_process_owner_death_and_abandoned_handoff_journal() {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    for phase in [
        "before-journal",
        "before-cas",
        "after-cas",
        "after-publication",
    ] {
        let backing = new_backing_id().unwrap();
        let key = Key::anonymous(backing, 0);
        let name = format!(
            r"Local\kinakaze.pi.test.{}.{}.{}",
            backing[0], backing[1], backing[2]
        );
        let (_section, view) = mapping(&name);
        let ready = Handle::new(unsafe {
            CreateEventW(ptr::null(), 0, 0, wide(&format!("{name}.ready")).as_ptr())
        })
        .unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "futex::pi::tests::handoff_child",
                "--ignored",
                "--nocapture",
            ])
            .env("KINAKAZE_PI_CRASH", phase)
            .env("KINAKAZE_PI_MAPPING", &name)
            .env(
                "KINAKAZE_PI_KEY",
                key.0.map(|part| part.to_string()).join(","),
            )
            .creation_flags(0x0800_0000)
            .spawn()
            .unwrap();
        assert_eq!(unsafe { WaitForSingleObject(ready.0, 5000) }, WAIT_OBJECT_0);
        let tid = crate::fsextra::kinakaze_abi_gettid() as u32;
        let park = park().unwrap();
        let mut transaction = Transaction::begin().unwrap();
        let Acquisition::Queued(token) =
            acquire(&mut transaction, key, view.Value as usize, tid, false).unwrap()
        else {
            panic!("owner did not retain the lock");
        };
        drop(transaction);
        let started = Instant::now();
        loop {
            let mut transaction = Transaction::begin().unwrap();
            if poll(&mut transaction, key, view.Value as usize, token, false).unwrap() {
                break;
            }
            let owner = owner_handle(&transaction, key);
            drop(transaction);
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "{phase}: stranded PI waiter"
            );
            if let Ok(owner) = owner {
                unsafe { kinakaze_vfs::deadline_wait::any(&[park, owner.raw()], 5000) };
            }
        }
        assert_eq!(child.wait().unwrap().code(), Some(77));
        let value = atomic_word::read(view.Value as usize).unwrap();
        assert_eq!(value & TID_MASK, tid, "{phase}");
        assert_eq!(
            value & OWNER_DIED != 0,
            phase == "before-journal",
            "{phase}"
        );
        let mut transaction = Transaction::begin().unwrap();
        unlock(&mut transaction, key, view.Value as usize, tid).unwrap();
        drop(transaction);
        assert_eq!(count_for_test(key), 0);
        unsafe { UnmapViewOfFile(view) };
    }
}

#[test]
fn pi_record_roles_leave_normal_wait_masks_and_key_layout_intact() {
    let identity = Identity::current().unwrap();
    let key = Key::private(0x4000).unwrap();
    let waiter = identity.record(key, 1, 42, WAIT | hybrid::PRIVATE_PARK);
    assert!(waiter.pi_wait());
    assert!(!waiter.metadata());
    assert!(identity.record(key, 1, 42, STATE).metadata());
    assert!(identity.record(key, 1, 42, JOURNAL).metadata());
    assert_eq!(size_of::<Record>(), 72);
}

#[test]
fn priority_mapping_donates_across_process_classes_without_lowering_base() {
    let identity = Identity::current().unwrap();
    for (class, base) in [(4, 0), (8, -2), (13, 2), (24, 0)] {
        let task = Task {
            identity,
            namespace: 1,
            tid: 42,
            base,
            applied: base,
            class,
        };
        for floor in [1, 6, 8, 10, 13, 15] {
            let relative = task.relative(floor);
            assert!(task.absolute(relative) >= task.absolute(base));
            assert!(task.absolute(relative) >= floor);
        }
    }
}

#[test]
fn pi_metadata_never_impersonates_a_retired_normal_or_vector_waiter() {
    let word = AtomicI32::new(0);
    let key = Key::private(word.as_ptr() as usize).unwrap();
    let metadata_key = Key::private(word.as_ptr() as usize + 8).unwrap();
    let identity = Identity::current().unwrap();
    for vector in [false, true] {
        let scalar = (!vector).then(|| Waiter::enqueue(0, u32::MAX, &|| Ok((key, 0))).unwrap());
        let mut group = vector.then(|| WaitGroup::new().unwrap());
        if let Some(group) = &mut group {
            group.enqueue(0, 0, || Ok((key, 0))).unwrap();
        }
        let mut transaction = Transaction::begin().unwrap();
        let token = transaction
            .records
            .iter()
            .find(|record| record.key == key && !record.metadata())
            .unwrap()
            .token;
        assert_ne!(token, 0);
        transaction.reserve(1).unwrap();
        transaction
            .records
            .push(identity.record(metadata_key, token, 42, STATE));
        transaction.dirty = true;
        transaction.commit();
        drop(transaction);
        assert_eq!(wake(key, 1, u32::MAX), Ok(1));
        if let Some(scalar) = &scalar {
            assert_eq!(scalar.finish(false), Ok(true));
        }
        if let Some(group) = &group {
            assert_eq!(group.finish(false), Ok(Some(0)));
        }
        let mut transaction = Transaction::begin().unwrap();
        assert!(!transaction.contains(token));
        assert!(
            transaction
                .records
                .iter()
                .any(|record| record.key == metadata_key && record.metadata())
        );
        transaction
            .records
            .retain(|record| record.key != metadata_key);
        transaction.dirty = true;
    }
}
