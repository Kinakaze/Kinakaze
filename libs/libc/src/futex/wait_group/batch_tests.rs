use super::*;
use std::os::windows::process::CommandExt;
use std::process::Command;

#[test]
fn vector_one_event_handles_128_duplicate_rows_requeue_and_retirement() {
    let key = Key::anonymous([800, process_birth().unwrap(), 1], 0);
    let other = Key::anonymous([800, process_birth().unwrap(), 2], 0);
    let indices: Vec<_> = (0..128).collect();
    let vector = WaitGroup::enqueue_batch(&indices, || Ok(vec![key; indices.len()])).unwrap();
    assert_eq!(vector.finish(false), Ok(None));
    assert_eq!(requeue(key, other, 0, 128, None), Ok(128));
    assert_eq!(wake(key, 1, u32::MAX), Ok(0));
    assert_eq!(wake(other, 1, u32::MAX), Ok(1));
    assert_eq!(vector.finish(false), Ok(Some(0)));
    assert_eq!(vector.finish(true), Ok(Some(0)));
    assert_eq!(wake(other, 128, u32::MAX), Ok(0));
    let canceled = WaitGroup::enqueue_batch(&[1, 127], || Ok(vec![key, other])).unwrap();
    assert_eq!(canceled.finish(true), Ok(None));
    assert_eq!(canceled.finish(false), Ok(None));
    assert_eq!(wake(key, 1, u32::MAX), Ok(0));
    assert_eq!(wake(other, 1, u32::MAX), Ok(0));
}

#[test]
#[ignore = "subprocess helper for vector wake/crash/domain tests"]
fn vector_child() {
    let key: Vec<u64> = std::env::var("KINAKAZE_FUTEX_VECTOR_KEY")
        .unwrap()
        .split(',')
        .map(|value| value.parse().unwrap())
        .collect();
    let key = Key(key.try_into().unwrap());
    let mode = std::env::var("KINAKAZE_FUTEX_VECTOR_MODE").unwrap();
    if mode == "other-domain" {
        kinakaze_runtime::authority::install_helper_domain(process_birth().unwrap()).unwrap();
        assert_eq!(wake(key, 1, u32::MAX), Ok(0));
    } else if mode == "wake" {
        assert_eq!(wake(key, 1, u32::MAX), Ok(1));
    } else {
        let shared = shared().unwrap();
        let _guard = shared.acquire().unwrap();
        let mut records = shared.load().unwrap();
        assert_eq!(shared.select(&mut records, key, 1, u32::MAX), Ok(1));
        if mode == "after-commit" {
            shared.commit(&records);
        } else if mode == "partial-bank" {
            let next = 1 - shared.header().active.load(Ordering::Acquire);
            unsafe { ptr::addr_of_mut!((*shared.bank(next)).count).write(u64::MAX) };
        }
        unsafe { windows_sys::Win32::System::Threading::ExitProcess(77) };
    }
}

#[test]
fn vector_cross_process_wake_and_crash_recovery_preserve_member_identity() {
    for mode in [
        "wake",
        "before-commit",
        "partial-bank",
        "after-commit",
        "other-domain",
    ] {
        let key = Key::anonymous([801, process_birth().unwrap(), 0], 0);
        let other = Key::anonymous([801, process_birth().unwrap(), 1], 0);
        let vector = WaitGroup::enqueue_batch(&[17, 127], || Ok(vec![other, key])).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "futex::wait_group::batch_tests::vector_child",
                "--ignored",
                "--nocapture",
            ])
            .env(
                "KINAKAZE_FUTEX_VECTOR_KEY",
                key.0.map(|part| part.to_string()).join(","),
            )
            .env("KINAKAZE_FUTEX_VECTOR_MODE", mode)
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(if matches!(mode, "wake" | "other-domain") {
                0
            } else {
                77
            }),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if !matches!(mode, "wake" | "after-commit") {
            assert_eq!(vector.finish(false), Ok(None), "{mode}");
            assert_eq!(wake(key, 1, u32::MAX), Ok(1));
        }
        assert_eq!(
            unsafe { WaitForSingleObject(vector.event(), 5000) },
            WAIT_OBJECT_0
        );
        assert_eq!(vector.finish(false), Ok(Some(127)), "{mode}");
        assert_eq!(wake(other, 1, u32::MAX), Ok(0));
        assert_eq!(wake(key, 1, u32::MAX), Ok(0));
    }
}

#[test]
fn vector_rejected_callback_leaves_no_records() {
    let key = Key::anonymous([802, process_birth().unwrap(), 0], 0);
    assert!(matches!(
        WaitGroup::enqueue_batch(&[0], || Err(EAGAIN)),
        Err(EAGAIN)
    ));
    assert_eq!(wake(key, 1, u32::MAX), Ok(0));
}
