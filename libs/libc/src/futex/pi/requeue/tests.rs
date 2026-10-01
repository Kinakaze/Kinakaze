use super::*;
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
    let name = std::env::var("KINAKAZE_REQUEUE_MAPPING").unwrap();
    let (_section, view) = mapping(&name);
    let values: Vec<u64> = std::env::var("KINAKAZE_REQUEUE_BACKING")
        .unwrap()
        .split(',')
        .map(|part| part.parse().unwrap())
        .collect();
    let backing: [u64; 3] = values.try_into().unwrap();
    let source = Key::anonymous(backing, 0);
    let target = Key::anonymous(backing, 4);
    let mut transaction = Transaction::begin().unwrap();
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
